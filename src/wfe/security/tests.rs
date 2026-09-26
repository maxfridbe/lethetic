use super::*;
#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

const TEST_NOW_UNIX: u64 = 1_800_000_000;

fn test_now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(TEST_NOW_UNIX)
}

#[test]
fn target_parser_accepts_only_strict_ip_and_dns_https_authorities() {
    let default = WfeTarget::parse(DEFAULT_WFE_TARGET).unwrap();
    assert_eq!(
        default.literal_socket_addr(),
        Some("127.0.0.1:11223".parse().unwrap())
    );

    let ipv6 = WfeTarget::parse("https://[::1]:8443").unwrap();
    assert_eq!(
        ipv6.ip_literal(),
        Some(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST))
    );
    let dns = WfeTarget::parse("https://brainiac:11223").unwrap();
    assert_eq!(dns.dns_name(), Some("brainiac"));
    assert_eq!(dns.ip_literal(), None);
    assert_eq!(dns.literal_socket_addr(), None);
    assert_eq!(dns.port(), 11223);
    assert_eq!(dns.canonical_origin(), "https://brainiac:11223");
    assert_eq!(dns.canonical_authority(), "brainiac:11223");
    assert!(dns.matches_origin_header("https://brainiac:11223"));
    assert!(dns.matches_host_header("brainiac:11223"));
    assert!(WfeTarget::parse("https://brainiac-nvidia:11223").is_ok());
    assert!(WfeTarget::parse("https://node.example.test:443").is_ok());
    assert!(WfeTarget::parse("https://127.0.0.1:443").is_ok());
    assert!(WfeTarget::parse("https://100.102.242.15:11223").is_ok());

    for mapped in [
        "https://[::ffff:0:0]:11223",
        "https://[::ffff:ffff:ffff]:11223",
        "https://[::ffff:e000:1]:11223",
        "https://[::ffff:6466:f20f]:11223",
    ] {
        let error = WfeTarget::parse(mapped).unwrap_err();
        assert!(error.contains("IPv4-mapped"), "{mapped}: {error}");
    }

    for rejected in [
        "http://127.0.0.1:11223",
        "https://Brainiac:11223",
        "https://brainiac.:11223",
        "https://brain_iac:11223",
        "https://*.example.test:11223",
        "https://-brainiac:11223",
        "https://brainiac-:11223",
        "https://br%C3%A4iniac:11223",
        "https://bräiniac:11223",
        "https://127.1:11223",
        "https://2130706433:11223",
        "https://12345:11223",
        "https://brainiac:011223",
        "https://brainiac",
        "https://0.0.0.0:11223",
        "https://[::]:11223",
        "https://255.255.255.255:11223",
        "https://224.0.0.1:11223",
        "https://[ff02::1]:11223",
        "https://[::ffff:100.102.242.15]:11223",
        "https://127.0.0.1",
        "https://127.0.0.1:0",
        "https://127.0.0.1:65536",
        "https://user@127.0.0.1:11223",
        "https://user:pass@127.0.0.1:11223",
        "https://127.0.0.1:11223/",
        "https://127.0.0.1:11223/path",
        "https://127.0.0.1:11223?token=x",
        "https://127.0.0.1:11223#token",
        " https://127.0.0.1:11223",
    ] {
        assert!(WfeTarget::parse(rejected).is_err(), "accepted {rejected:?}");
    }
}

#[test]
fn dns_target_name_boundaries_are_exact() {
    let maximum = [
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61),
    ]
    .join(".");
    assert_eq!(maximum.len(), 253);
    assert!(WfeTarget::parse(&format!("https://{maximum}:11223")).is_ok());

    let oversized_total = [
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(62),
    ]
    .join(".");
    assert_eq!(oversized_total.len(), 254);
    assert!(WfeTarget::parse(&format!("https://{oversized_total}:11223")).is_err());
    assert!(WfeTarget::parse(&format!("https://{}.example:11223", "a".repeat(64))).is_err());
}

#[test]
fn tls_and_authentication_selection_are_orthogonal_and_fail_closed() {
    assert!(matches!(
        select_profile(None, None).unwrap(),
        ProfileSelection::Automatic
    ));
    assert!(select_profile(Some("cert".into()), None).is_err());
    assert!(select_profile(None, Some("key".into())).is_err());
    assert!(matches!(
        select_profile(Some("cert".into()), Some("key".into())).unwrap(),
        ProfileSelection::Explicit(_)
    ));

    assert!(matches!(
        select_authentication(false, None, ControllerAuthenticationMode::TokenRequired).unwrap(),
        AuthenticationSelection::GenerateToken
    ));
    assert!(matches!(
        select_authentication(
            true,
            Some("token".into()),
            ControllerAuthenticationMode::TokenRequired,
        )
        .unwrap(),
        AuthenticationSelection::ExplicitToken(_)
    ));
    assert!(matches!(
        select_authentication(false, None, ControllerAuthenticationMode::Disabled).unwrap(),
        AuthenticationSelection::Disabled
    ));
    assert!(matches!(
        select_authentication(true, None, ControllerAuthenticationMode::Disabled).unwrap(),
        AuthenticationSelection::Disabled
    ));
    assert!(
        select_authentication(
            false,
            Some("token".into()),
            ControllerAuthenticationMode::TokenRequired,
        )
        .is_err()
    );
    assert!(
        select_authentication(true, None, ControllerAuthenticationMode::TokenRequired).is_err()
    );
    assert!(
        select_authentication(
            true,
            Some("token".into()),
            ControllerAuthenticationMode::Disabled,
        )
        .is_err()
    );
}

#[test]
fn generated_identity_paths_are_canonical_per_identity() {
    assert_eq!(
        generated_identity_directory_name(&WfeHostIdentity::Ip("100.102.242.15".parse().unwrap())),
        "ipv4-6466f20f"
    );
    assert_eq!(
        generated_identity_directory_name(&WfeHostIdentity::Ip("2001:db8::1".parse().unwrap())),
        "ipv6-20010db8000000000000000000000001"
    );
    assert_eq!(
        generated_identity_directory_name(&WfeHostIdentity::Dns("brainiac".to_string())),
        "dns-0b8c619c74147c304aa760417a5a76cc4ad72653c9880d704a0978be8243aa15"
    );
}

#[test]
fn exact_origin_compares_only_scheme_host_and_effective_port() {
    let target = WfeTarget::parse("https://127.0.0.1:443").unwrap();
    assert_eq!(target.canonical_origin(), "https://127.0.0.1");
    assert_eq!(target.canonical_authority(), "127.0.0.1");
    assert!(target.matches_origin_header("https://127.0.0.1"));
    assert!(!target.matches_origin_header("https://127.0.0.1/"));
    assert!(target.matches_host_header("127.0.0.1"));
    assert!(!target.matches_host_header("127.0.0.1:443"));
    assert!(target.has_exact_origin_str("https://127.0.0.1/path?ignored=yes"));
    assert!(target.has_exact_origin_str("https://127.0.0.1:443"));
    assert!(!target.has_exact_origin_str("http://127.0.0.1:443"));
    assert!(!target.has_exact_origin_str("https://127.0.0.2:443"));
    assert!(!target.has_exact_origin_str("https://127.0.0.1:444"));
    assert!(!target.has_exact_origin_str("not a url"));
}

#[test]
fn token_validation_requires_url_safe_base64_entropy_and_one_trailing_lf() {
    let token = URL_SAFE_NO_PAD.encode([7_u8; MIN_TOKEN_DECODED_BYTES]);
    let parsed = parse_controller_secret(token.as_bytes()).unwrap();
    assert!(parsed.verify(&token));
    assert!(!parsed.verify("different"));
    assert!(!format!("{parsed:?}").contains(&token));

    let with_lf = format!("{token}\n");
    assert!(parse_controller_secret(with_lf.as_bytes()).is_ok());
    assert!(parse_controller_secret(format!("\n{token}").as_bytes()).is_err());
    assert!(parse_controller_secret(format!("{token}\n\n").as_bytes()).is_err());
    assert!(parse_controller_secret(b"not+url/safe").is_err());
    let short = URL_SAFE_NO_PAD.encode([1_u8; MIN_TOKEN_DECODED_BYTES - 1]);
    assert!(parse_controller_secret(short.as_bytes()).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn generated_identity_is_private_exact_ip_reused_and_token_is_process_local() {
    use std::os::unix::fs::MetadataExt;

    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("wfe-state");
    let target = WfeTarget::parse("https://100.102.242.15:11223").unwrap();
    let first = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();
    assert_eq!(first.profile(), SecurityProfile::AutomaticGenerated);
    assert_eq!(
        first.controller_authentication_mode(),
        ControllerAuthenticationMode::TokenRequired
    );
    assert_eq!(first.fingerprint_sha256().len(), 64);
    let first_token = first.bootstrap_token_for_host().unwrap().to_string();
    assert!(first.verify_controller_token(&first_token));
    assert!(!format!("{first:?}").contains(&first_token));

    let leaf = parse_single_certificate(&first.certificate_chain_der()[0]).unwrap();
    let san = leaf.subject_alternative_name().unwrap().unwrap();
    assert_eq!(san.value.general_names.len(), 1);
    assert!(matches!(
        &san.value.general_names[0],
        GeneralName::IPAddress(bytes) if ip_san_matches(bytes, target.ip_literal().unwrap())
    ));

    let identity_root = generated_identity_state_root(&state_root, &target.identity);
    assert_eq!(fs::metadata(&state_root).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(fs::metadata(&identity_root).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(identity_root.file_name().unwrap(), "ipv4-6466f20f");

    let mut names = fs::read_dir(&identity_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            GENERATED_IDENTITY_LOCK_FILE.to_string(),
            GENERATED_CERTIFICATE_FILE.to_string(),
            GENERATED_MANIFEST_FILE.to_string(),
            GENERATED_PRIVATE_KEY_FILE.to_string(),
        ]
    );
    for name in &names {
        let path = identity_root.join(name);
        let metadata = fs::metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        let bytes = fs::read(path).unwrap();
        assert!(
            !bytes
                .windows(first_token.len())
                .any(|part| part == first_token.as_bytes())
        );
    }

    let same_ip_other_port = WfeTarget::parse("https://100.102.242.15:443").unwrap();
    let second = prepare_security_at(
        &same_ip_other_port,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now() + Duration::from_secs(60),
    )
    .unwrap();
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert_ne!(first_token, second.bootstrap_token_for_host().unwrap());

    let manifest: GeneratedIpManifest =
        serde_json::from_slice(&fs::read(identity_root.join(GENERATED_MANIFEST_FILE)).unwrap())
            .unwrap();
    assert_eq!(manifest.version, IP_MANIFEST_VERSION);
    assert_eq!(manifest.target_ip, target.ip_literal().unwrap().to_string());

    let tokenless = prepare_security_at_with_authentication(
        &target,
        SecurityFileOptions::default(),
        ControllerAuthenticationMode::Disabled,
        Some(&state_root),
        test_now() + Duration::from_secs(120),
    )
    .unwrap();
    assert_eq!(tokenless.profile(), SecurityProfile::AutomaticGenerated);
    assert_eq!(
        tokenless.controller_authentication_mode(),
        ControllerAuthenticationMode::Disabled
    );
    assert_eq!(tokenless.fingerprint(), first.fingerprint());
    assert!(tokenless.bootstrap_token_for_host().is_none());
    assert!(!tokenless.verify_controller_token(&first_token));
}

#[cfg(target_os = "linux")]
#[test]
fn generated_dns_identity_has_one_san_and_reuses_only_the_exact_name() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("wfe-state");
    let target = WfeTarget::parse("https://brainiac:11223").unwrap();
    let first = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();
    let leaf = parse_single_certificate(&first.certificate_chain_der()[0]).unwrap();
    let san = leaf.subject_alternative_name().unwrap().unwrap();
    assert_eq!(san.value.general_names.len(), 1);
    assert!(matches!(
        &san.value.general_names[0],
        GeneralName::DNSName(name) if *name == "brainiac"
    ));

    let identity_root = generated_identity_state_root(&state_root, &target.identity);
    assert_eq!(
        identity_root.file_name().unwrap(),
        "dns-0b8c619c74147c304aa760417a5a76cc4ad72653c9880d704a0978be8243aa15"
    );
    let manifest: GeneratedDnsManifest =
        serde_json::from_slice(&fs::read(identity_root.join(GENERATED_MANIFEST_FILE)).unwrap())
            .unwrap();
    assert_eq!(manifest.version, DNS_MANIFEST_VERSION);
    assert!(matches!(
        manifest.identity,
        GeneratedDnsManifestIdentity::Dns { hostname } if hostname == "brainiac"
    ));

    let other_port = WfeTarget::parse("https://brainiac:443").unwrap();
    let reused = prepare_security_at(
        &other_port,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(first.fingerprint(), reused.fingerprint());

    let alias = WfeTarget::parse("https://brainiac-alias:11223").unwrap();
    let isolated = prepare_security_at(
        &alias,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();
    assert_ne!(first.fingerprint(), isolated.fingerprint());

    let literal = WfeTarget::parse("https://127.0.0.1:11223").unwrap();
    let literal_security = prepare_security_at(
        &literal,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();
    assert_ne!(first.fingerprint(), literal_security.fingerprint());

    let mut mismatched: serde_json::Value =
        serde_json::from_slice(&fs::read(identity_root.join(GENERATED_MANIFEST_FILE)).unwrap())
            .unwrap();
    mismatched["identity"]["hostname"] = serde_json::json!("brainiac-alias");
    write_test_private_file(
        &identity_root.join(GENERATED_MANIFEST_FILE),
        &serde_json::to_vec(&mismatched).unwrap(),
    );
    let rotated = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now() + Duration::from_secs(2),
    )
    .unwrap();
    assert_ne!(first.fingerprint(), rotated.fingerprint());
}

#[cfg(target_os = "linux")]
#[test]
fn generated_manifest_mismatch_and_expiry_window_rotate_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("wfe-state");
    let target = WfeTarget::parse(DEFAULT_WFE_TARGET).unwrap();
    let first = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();

    let identity_root = generated_identity_state_root(&state_root, &target.identity);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(identity_root.join(GENERATED_MANIFEST_FILE)).unwrap())
            .unwrap();
    manifest["target_ip"] = serde_json::json!("127.0.0.9");
    write_test_private_file(
        &identity_root.join(GENERATED_MANIFEST_FILE),
        &serde_json::to_vec(&manifest).unwrap(),
    );
    let rotated = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_ne!(first.fingerprint(), rotated.fingerprint());

    let near_expiry = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now()
            + Duration::from_secs(
                (GENERATED_CERTIFICATE_LIFETIME_SECONDS
                    - GENERATED_CERTIFICATE_ROTATE_BEFORE_SECONDS
                    + 1) as u64,
            ),
    )
    .unwrap();
    assert_ne!(rotated.fingerprint(), near_expiry.fingerprint());
}

#[cfg(target_os = "linux")]
#[test]
fn generated_identities_are_isolated_by_ip_and_support_ipv6() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("wfe-state");
    let first_target = WfeTarget::parse("https://100.102.242.15:11223").unwrap();
    let second_target = WfeTarget::parse("https://100.102.242.16:11223").unwrap();
    let ipv6_target = WfeTarget::parse("https://[2001:db8::1]:11223").unwrap();

    let first = prepare_security_at(
        &first_target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();
    let second = prepare_security_at(
        &second_target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();
    let ipv6 = prepare_security_at(
        &ipv6_target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();

    assert_ne!(first.fingerprint(), second.fingerprint());
    assert_ne!(first.fingerprint(), ipv6.fingerprint());
    assert_ne!(second.fingerprint(), ipv6.fingerprint());
    for target in [&first_target, &second_target, &ipv6_target] {
        assert!(generated_identity_state_root(&state_root, &target.identity).is_dir());
    }

    let first_again = prepare_security_at(
        &first_target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(first.fingerprint(), first_again.fingerprint());
}

#[cfg(unix)]
#[test]
fn explicit_failure_never_falls_back_to_generated_state() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("generated-state");
    let missing = temporary.path().join("missing");
    let target = WfeTarget::parse("https://100.102.242.15:11223").unwrap();
    let error = prepare_security_at(
        &target,
        SecurityFileOptions {
            cert: Some(missing.join("certificate")),
            key: Some(missing.join("private-key")),
            token: Some(missing.join("token")),
        },
        Some(&state_root),
        test_now(),
    )
    .unwrap_err();
    assert!(error.contains("certificate"), "{error}");
    assert!(!state_root.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn legacy_flat_loopback_state_is_left_inert() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("wfe-state");
    ensure_private_state_root(&state_root).unwrap();
    let legacy_files = [
        (GENERATED_CERTIFICATE_FILE, b"legacy-certificate".as_slice()),
        (GENERATED_PRIVATE_KEY_FILE, b"legacy-private-key".as_slice()),
        (GENERATED_MANIFEST_FILE, b"legacy-manifest".as_slice()),
    ];
    for (name, bytes) in legacy_files {
        write_test_private_file(&state_root.join(name), bytes);
    }

    let target = WfeTarget::parse(DEFAULT_WFE_TARGET).unwrap();
    let prepared = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now(),
    )
    .unwrap();
    assert_eq!(prepared.profile(), SecurityProfile::AutomaticGenerated);
    for (name, bytes) in legacy_files {
        assert_eq!(fs::read(state_root.join(name)).unwrap(), bytes);
    }
    assert!(generated_identity_state_root(&state_root, &target.identity).is_dir());
}

#[cfg(target_os = "linux")]
#[test]
fn concurrent_same_ip_generation_converges_on_one_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let state_root = temporary.path().join("wfe-state");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let mut workers = Vec::new();
    for _ in 0..8 {
        let state_root = state_root.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            let target = WfeTarget::parse("https://100.102.242.15:11223").unwrap();
            barrier.wait();
            prepare_security_at(
                &target,
                SecurityFileOptions::default(),
                Some(&state_root),
                test_now(),
            )
            .unwrap()
            .fingerprint()
        }));
    }
    let fingerprints = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert!(
        fingerprints
            .iter()
            .all(|fingerprint| *fingerprint == fingerprints[0])
    );

    let target = WfeTarget::parse("https://100.102.242.15:443").unwrap();
    let reused = prepare_security_at(
        &target,
        SecurityFileOptions::default(),
        Some(&state_root),
        test_now() + Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(reused.fingerprint(), fingerprints[0]);
}

#[cfg(unix)]
#[test]
fn explicit_identity_and_token_are_fully_validated() {
    let temporary = tempfile::tempdir().unwrap();
    let target = WfeTarget::parse("https://127.0.0.2:9443").unwrap();
    let (cert, key) = make_test_identity(
        target.ip_literal().unwrap(),
        test_now(),
        IsCa::ExplicitNoCa,
        vec![ExtendedKeyUsagePurpose::ServerAuth],
        vec![SanType::IpAddress(target.ip_literal().unwrap())],
    );
    let cert_path = temporary.path().join("cert.der");
    let key_path = temporary.path().join("key.der");
    let token_path = temporary.path().join("token");
    let token = URL_SAFE_NO_PAD.encode([42_u8; MIN_TOKEN_DECODED_BYTES]);
    write_test_private_file(&cert_path, &cert);
    write_test_private_file(&key_path, &key);
    write_test_private_file(&token_path, format!("{token}\n").as_bytes());

    let prepared = prepare_security_at(
        &target,
        SecurityFileOptions {
            cert: Some(cert_path.clone()),
            key: Some(key_path.clone()),
            token: Some(token_path),
        },
        None,
        test_now(),
    )
    .unwrap();
    assert_eq!(prepared.profile(), SecurityProfile::ExplicitFiles);
    assert_eq!(
        prepared.controller_authentication_mode(),
        ControllerAuthenticationMode::TokenRequired
    );
    assert!(prepared.verify_controller_token(&token));
    assert_eq!(prepared.bootstrap_token_for_host(), None);
    assert!(!format!("{prepared:?}").contains(&token));

    let tokenless = prepare_security_at_with_authentication(
        &target,
        SecurityFileOptions {
            cert: Some(cert_path),
            key: Some(key_path),
            token: None,
        },
        ControllerAuthenticationMode::Disabled,
        None,
        test_now(),
    )
    .unwrap();
    assert_eq!(tokenless.profile(), SecurityProfile::ExplicitFiles);
    assert_eq!(
        tokenless.controller_authentication_mode(),
        ControllerAuthenticationMode::Disabled
    );
    assert!(tokenless.bootstrap_token_for_host().is_none());
    assert!(!tokenless.verify_controller_token(&token));
}

#[test]
fn dns_identity_requires_one_exact_dns_san() {
    let target = WfeTarget::parse("https://brainiac:11223").unwrap();
    for candidate in ["brainiac", "BRAINIAC"] {
        let (cert, key) = make_test_identity_with_common_name(
            "ignored-common-name",
            test_now(),
            IsCa::ExplicitNoCa,
            vec![ExtendedKeyUsagePurpose::ServerAuth],
            vec![SanType::DnsName(candidate.try_into().unwrap())],
        );
        let chain = vec![CertificateDer::from(cert)];
        validate_certificate_chain(&chain, &target.identity, test_now()).unwrap();
        let key = parse_private_key(&key).unwrap();
        build_server_config(&chain, &key, &target.identity).unwrap();
    }

    let cases = [
        vec![SanType::DnsName("other-host".try_into().unwrap())],
        vec![SanType::DnsName("*.example.test".try_into().unwrap())],
        vec![SanType::DnsName("brainiac.".try_into().unwrap())],
        vec![SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST))],
        vec![
            SanType::DnsName("brainiac".try_into().unwrap()),
            SanType::DnsName("other-host".try_into().unwrap()),
        ],
        Vec::new(),
    ];
    for sans in cases {
        let (cert, _) = make_test_identity_with_common_name(
            "brainiac",
            test_now(),
            IsCa::ExplicitNoCa,
            vec![ExtendedKeyUsagePurpose::ServerAuth],
            sans,
        );
        let error =
            validate_certificate_chain(&[CertificateDer::from(cert)], &target.identity, test_now())
                .unwrap_err();
        assert!(error.contains("SAN"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn explicit_identity_rejects_wrong_san_ca_eku_expiry_and_key_mismatch() {
    let target = WfeTarget::parse("https://127.0.0.2:9443").unwrap();
    let cases = [
        (
            IsCa::ExplicitNoCa,
            vec![ExtendedKeyUsagePurpose::ServerAuth],
            vec![SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST))],
            "SAN",
        ),
        (
            IsCa::Ca(rcgen::BasicConstraints::Unconstrained),
            vec![ExtendedKeyUsagePurpose::ServerAuth],
            vec![SanType::IpAddress(target.ip_literal().unwrap())],
            "non-CA",
        ),
        (
            IsCa::ExplicitNoCa,
            vec![ExtendedKeyUsagePurpose::ClientAuth],
            vec![SanType::IpAddress(target.ip_literal().unwrap())],
            "server-auth",
        ),
        (
            IsCa::ExplicitNoCa,
            vec![ExtendedKeyUsagePurpose::ServerAuth],
            vec![
                SanType::IpAddress(target.ip_literal().unwrap()),
                SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            ],
            "exactly",
        ),
    ];
    for (is_ca, eku, sans, expected) in cases {
        let (cert, _) =
            make_test_identity(target.ip_literal().unwrap(), test_now(), is_ca, eku, sans);
        let chain = vec![CertificateDer::from(cert)];
        let error = validate_certificate_chain(&chain, &target.identity, test_now()).unwrap_err();
        assert!(error.contains(expected), "{error:?}");
    }

    let (cert, _) = make_test_identity(
        target.ip_literal().unwrap(),
        test_now(),
        IsCa::ExplicitNoCa,
        vec![ExtendedKeyUsagePurpose::ServerAuth],
        vec![SanType::IpAddress(target.ip_literal().unwrap())],
    );
    let (_, wrong_key) = make_test_identity(
        target.ip_literal().unwrap(),
        test_now(),
        IsCa::ExplicitNoCa,
        vec![ExtendedKeyUsagePurpose::ServerAuth],
        vec![SanType::IpAddress(target.ip_literal().unwrap())],
    );
    let chain = vec![CertificateDer::from(cert)];
    let key = parse_private_key(&wrong_key).unwrap();
    assert!(build_server_config(&chain, &key, &target.identity).is_err());

    let (expired_cert, _) = make_test_identity(
        target.ip_literal().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        IsCa::ExplicitNoCa,
        vec![ExtendedKeyUsagePurpose::ServerAuth],
        vec![SanType::IpAddress(target.ip_literal().unwrap())],
    );
    assert!(
        validate_certificate_chain(
            &[CertificateDer::from(expired_cert)],
            &target.identity,
            test_now(),
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn explicit_files_reject_symlinks_hardlinks_insecure_modes_and_oversize() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("original");
    write_test_private_file(&original, b"private");

    let symlink_path = temporary.path().join("link");
    symlink(&original, &symlink_path).unwrap();
    assert!(read_explicit_private_file(&symlink_path, 64, "test").is_err());

    let hardlink_path = temporary.path().join("hardlink");
    fs::hard_link(&original, &hardlink_path).unwrap();
    assert!(read_explicit_private_file(&original, 64, "test").is_err());
    fs::remove_file(hardlink_path).unwrap();

    fs::set_permissions(&original, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_explicit_private_file(&original, 64, "test").is_err());
    fs::set_permissions(&original, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(read_explicit_private_file(&original, 3, "test").is_err());
}

fn make_test_identity(
    ip: IpAddr,
    now: SystemTime,
    is_ca: IsCa,
    extended_key_usages: Vec<ExtendedKeyUsagePurpose>,
    sans: Vec<SanType>,
) -> (Vec<u8>, Vec<u8>) {
    make_test_identity_with_common_name(&ip.to_string(), now, is_ca, extended_key_usages, sans)
}

fn make_test_identity_with_common_name(
    common_name: &str,
    now: SystemTime,
    is_ca: IsCa,
    extended_key_usages: Vec<ExtendedKeyUsagePurpose>,
    sans: Vec<SanType>,
) -> (Vec<u8>, Vec<u8>) {
    let now = system_time_to_unix(now).unwrap();
    let mut parameters = CertificateParams::default();
    parameters.not_before = ASN1Time::from_timestamp(now - 60).unwrap().to_datetime();
    parameters.not_after = ASN1Time::from_timestamp(now + 3600).unwrap().to_datetime();
    parameters.subject_alt_names = sans;
    parameters.is_ca = is_ca;
    parameters.extended_key_usages = extended_key_usages;
    parameters.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, common_name);
    parameters.distinguished_name = name;
    let key = Zeroizing::new(KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap());
    let cert = parameters.self_signed(&*key).unwrap();
    (cert.der().as_ref().to_vec(), key.serialize_der())
}

#[cfg(unix)]
fn write_test_private_file(path: &Path, bytes: &[u8]) {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .unwrap();
    file.sync_all().unwrap();
}
