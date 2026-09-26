use super::{adapter::*, broker::*, network_policy::*, peer::*, protocol::*, relay::*};
use base64::Engine as _;
use serde_json::{Value, json};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt, duplex};
use tokio::net::{TcpListener, TcpStream, UnixListener, UnixStream};
use tokio::process::Command;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

fn test_capability() -> String {
    "ab".repeat(CAPABILITY_BYTES)
}

#[test]
fn broker_config_keeps_label_disabled_and_labeled_profiles_distinct() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let directory = directory.path().canonicalize().unwrap();
    let base = BrokerConfig {
        socket_path: directory.join("broker.sock"),
        audit_path: directory.join("audit.jsonl"),
        runtime_id: "runtime-1".to_string(),
        capability: test_capability(),
        expected_peer_uid: rustix::process::geteuid().as_raw(),
        expected_peer_gid: rustix::process::getegid().as_raw(),
        derive_peer_credentials: false,
        expected_peer_pid: std::process::id(),
        selinux_labels: None,
        allow_label_disabled_peer: true,
        max_connections: 1,
    };
    base.validate().unwrap();

    let labels = crate::python::selinux::SelinuxLabels::new(
        "system_u:system_r:container_t:s0:c1,c2".to_string(),
        "system_u:object_r:container_file_t:s0:c1,c2".to_string(),
    )
    .unwrap();
    let invalid = BrokerConfig {
        selinux_labels: Some(labels.clone()),
        ..base.clone()
    };
    assert_eq!(
        invalid.validate().unwrap_err(),
        "label-disabled broker peer mode cannot also require SELinux labels"
    );

    BrokerConfig {
        selinux_labels: Some(labels),
        allow_label_disabled_peer: false,
        ..base
    }
    .validate()
    .unwrap();
}

fn proxy_request(head: &str) -> BrokerRequest {
    BrokerRequest {
        version: BROKER_PROTOCOL_VERSION,
        runtime_id: "runtime-1".to_string(),
        capability: test_capability(),
        kind: BrokerRequestKind::Proxy,
        proxy_head_base64: Some(base64::engine::general_purpose::STANDARD.encode(head.as_bytes())),
        buffered_after_head: false,
    }
}

fn routes() -> RouteSnapshot {
    RouteSnapshot {
        nondefault: vec![
            IpPrefix::parse("10.0.0.0/8", AddressFamily::V4).unwrap(),
            IpPrefix::parse("203.0.113.0/24", AddressFamily::V4).unwrap(),
            IpPrefix::parse("2001:db8::/32", AddressFamily::V6).unwrap(),
        ],
        local_addresses: [
            "8.8.4.4".parse().unwrap(),
            "2606:4700:4700::1111".parse().unwrap(),
        ]
        .into_iter()
        .collect(),
        has_default_v4: true,
        has_default_v6: true,
    }
}

#[test]
fn rejects_ipv4_special_ranges_at_boundaries() {
    let denied = [
        "0.0.0.0",
        "0.255.255.255",
        "10.0.0.1",
        "100.64.0.0",
        "100.127.255.255",
        "127.0.0.1",
        "169.254.169.254",
        "172.16.0.0",
        "172.31.255.255",
        "192.0.0.192",
        "192.0.2.1",
        "192.31.196.1",
        "192.52.193.1",
        "192.88.99.1",
        "192.175.48.1",
        "192.168.1.1",
        "198.18.0.0",
        "198.19.255.255",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "239.255.255.255",
        "240.0.0.1",
        "255.255.255.255",
    ];
    for address in denied {
        assert!(
            !is_public_destination(address.parse().unwrap()),
            "{address}"
        );
    }
    for address in [
        "9.255.255.255",
        "11.0.0.0",
        "100.63.255.255",
        "100.128.0.0",
        "172.15.255.255",
        "172.32.0.0",
        "192.167.255.255",
        "192.169.0.0",
        "198.17.255.255",
        "198.20.0.0",
        "203.0.112.255",
        "203.0.114.0",
        "223.255.255.255",
    ] {
        assert!(is_public_destination(address.parse().unwrap()), "{address}");
    }
}

#[test]
fn rejects_ipv6_local_transition_and_documentation_ranges() {
    for address in [
        "::",
        "::1",
        "::ffff:127.0.0.1",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "64:ff9b:1::1",
        "100::1",
        "2001::1",
        "2001:db8::1",
        "2002:0808:0808::1",
        "3fff::1",
        "5f00::1",
        "fc00::1",
        "fdff::1",
        "fe80::1",
        "fec0::1",
        "ff02::1",
    ] {
        assert!(
            !is_public_destination(address.parse().unwrap()),
            "{address}"
        );
    }
    for address in ["2000::1", "2606:4700:4700::1111", "2fff:ffff::1"] {
        assert!(is_public_destination(address.parse().unwrap()), "{address}");
    }
}

#[test]
fn ipv6_special_prefix_boundaries_are_exact() {
    for address in [
        "2000:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2001:200::",
        "2001:db7:ffff:ffff:ffff:ffff:ffff:ffff",
        "2001:db9::",
        "2001:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2003::",
        "3ffe:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "3fff:1000::",
        "2620:4f:7fff:ffff:ffff:ffff:ffff:ffff",
        "2620:4f:8001::",
    ] {
        assert!(is_public_destination(address.parse().unwrap()), "{address}");
    }
    for address in [
        "2001::",
        "2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2001:db8::",
        "2001:db8:ffff:ffff:ffff:ffff:ffff:ffff",
        "2002::",
        "2002:ffff:ffff:ffff:ffff:ffff:ffff:ffff",
        "3fff::",
        "3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff",
        "2620:4f:8000::",
        "2620:4f:8000:ffff:ffff:ffff:ffff:ffff",
    ] {
        assert!(
            !is_public_destination(address.parse().unwrap()),
            "{address}"
        );
    }
}

#[test]
fn hostname_parser_rejects_local_numeric_and_ambiguous_forms() {
    for host in [
        "localhost",
        "service.local",
        "service.internal",
        "example.com.",
        "EXAMPLE.com",
        "127.0.0.1",
        "2130706433",
        "0x7f000001",
        "017700000001",
        "0177.0.0.1",
        "127.1",
        "127.0.1",
        "127.000.000.001",
        "+127.0.0.1",
        "4294967297",
        "127%2e0%2e0%2e1",
        "user@example.com",
        "[::1]",
        "fe80::1%eth0",
        "singlelabel",
        "bad_label.example",
        "-bad.example",
        "bad-.example",
        "example.123",
    ] {
        assert!(validate_public_hostname(host).is_err(), "{host}");
    }
    for host in ["example.com", "cdn.example.org", "xn--bcher-kva.com"] {
        assert!(validate_public_hostname(host).is_ok(), "{host}");
    }
}

#[test]
fn mixed_dns_or_specific_routes_fail_closed() {
    let routes = routes();
    assert!(
        validate_dns_answers(
            ["8.8.8.8".parse().unwrap(), "127.0.0.1".parse().unwrap()],
            &routes
        )
        .is_err()
    );
    assert!(validate_dns_answers(["203.0.113.8".parse().unwrap()], &routes).is_err());
    assert!(validate_dns_answers(["8.8.4.4".parse().unwrap()], &routes).is_err());
    assert_eq!(
        validate_dns_answers(["8.8.8.8".parse().unwrap()], &routes).unwrap(),
        vec!["8.8.8.8".parse::<IpAddr>().unwrap()]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn netlink_monitor_ignores_neighbor_churn_but_invalidates_routes() {
    fn message(message_type: u16, family: u8) -> Vec<u8> {
        let mut bytes = vec![0_u8; 28];
        bytes[..4].copy_from_slice(&(28_u32).to_ne_bytes());
        bytes[4..6].copy_from_slice(&message_type.to_ne_bytes());
        bytes[16] = family;
        bytes
    }

    assert!(
        !netlink_invalidates_policy(&message(libc::RTM_NEWNEIGH, libc::AF_INET as u8,)).unwrap()
    );
    assert!(
        !netlink_invalidates_policy(&message(libc::RTM_NEWNEIGH, libc::AF_BRIDGE as u8,)).unwrap()
    );
    assert!(netlink_invalidates_policy(&message(libc::RTM_NEWRULE, libc::AF_INET as u8,)).unwrap());
    assert!(netlink_invalidates_policy(&message(104, libc::AF_UNSPEC as u8,)).unwrap());
    assert!(netlink_invalidates_policy(&[1, 2, 3]).is_err());
}

#[test]
fn dns_answer_limit_is_checked_before_policy_filtering() {
    let allowed = (0..MAX_DNS_ANSWERS)
        .map(|index| SocketAddr::from((Ipv4Addr::new(8, 8, 8, index as u8), 443)))
        .collect::<Vec<_>>();
    assert_eq!(collect_dns_answers(allowed).unwrap().len(), MAX_DNS_ANSWERS);
    let too_many = (0..=MAX_DNS_ANSWERS)
        .map(|index| SocketAddr::from((Ipv4Addr::new(8, 8, 8, index as u8), 443)))
        .collect::<Vec<_>>();
    assert!(collect_dns_answers(too_many).is_err());
}

#[test]
fn unroutable_family_is_discarded_without_weakening_mixed_answer_checks() {
    let mut routes = routes();
    routes.has_default_v6 = false;
    let answers = validate_dns_answers(
        [
            "2606:4700:4700::1001".parse().unwrap(),
            "8.8.8.8".parse().unwrap(),
        ],
        &routes,
    )
    .unwrap();
    assert_eq!(answers, vec!["8.8.8.8".parse::<IpAddr>().unwrap()]);
    assert!(
        validate_dns_answers(
            ["fc00::1".parse().unwrap(), "8.8.8.8".parse().unwrap()],
            &routes
        )
        .is_err()
    );
}

#[test]
fn route_json_tracks_defaults_specific_routes_and_interfaces() {
    let mut snapshot = RouteSnapshot::default();
    snapshot
        .parse_routes(
            &json!([
                {"dst":"default","dev":"eth0"},
                {"dst":"198.18.0.0/15","dev":"tun0"},
                {"type":"local","dst":"8.8.4.4","dev":"lo"}
            ]),
            AddressFamily::V4,
        )
        .unwrap();
    snapshot
        .parse_addresses(&json!([
            {"ifname":"eth0","addr_info":[{"local":"192.168.1.5"}]},
            {"ifname":"tun0","addr_info":[{"local":"100.64.0.2"}]}
        ]))
        .unwrap();
    assert!(snapshot.has_default_v4);
    assert_eq!(
        snapshot.classify("8.8.8.8".parse().unwrap()),
        RouteDecision::Allowed
    );
    assert_eq!(
        snapshot.classify("198.18.1.1".parse().unwrap()),
        RouteDecision::LocalOrSpecificRoute
    );
    assert_eq!(
        snapshot.classify("192.168.1.5".parse().unwrap()),
        RouteDecision::LocalOrSpecificRoute
    );
}

#[test]
fn default_tunnel_and_ambiguous_routes_fail_closed() {
    for route in [
        json!([{"dst":"default","dev":"tun0","type":"unicast"}]),
        json!([{"dst":"default","dev":"wg0"}]),
        json!([{"dst":"default","dev":"veth0"}]),
        json!([{"dst":"default"}]),
        json!([{"dst":"default","dev":"eth0","nexthops":[{"dev":"eth0"}]}]),
        json!([{"dst":"default","gateway":"192.168.1.1","dev":"eth0","encap":{"type":"ip","id":7}}]),
        json!([{"dst":"default","gateway":"192.168.1.1","dev":"eth0","nhid":42}]),
    ] {
        let mut snapshot = RouteSnapshot::default();
        assert!(
            snapshot.parse_routes(&route, AddressFamily::V4).is_err(),
            "{route}"
        );
    }
}

#[test]
fn parses_and_rewrites_plain_http_proxy_request() {
    let request = b"GET http://packages.example.org/index?q=rust HTTP/1.1\r\nHost: packages.example.org\r\nUser-Agent: test\r\nProxy-Connection: keep-alive\r\n\r\n";
    let intent = parse_proxy_request(request).unwrap();
    assert_eq!(intent.kind, ProxyKind::Http);
    assert_eq!(intent.host, "packages.example.org");
    assert_eq!(intent.port, 80);
    let rewritten = String::from_utf8(intent.rewritten_head.unwrap()).unwrap();
    assert!(rewritten.starts_with("GET /index?q=rust HTTP/1.1\r\n"));
    assert!(rewritten.contains("Host: packages.example.org\r\n"));
    assert!(rewritten.contains("User-Agent: test\r\n"));
    assert!(!rewritten.to_ascii_lowercase().contains("proxy-connection"));
    assert!(rewritten.ends_with("Connection: close\r\n\r\n"));
}

#[test]
fn parses_connect_only_for_public_dns_and_port_443() {
    let intent = parse_proxy_request(
        b"CONNECT crates.example.org:443 HTTP/1.1\r\nHost: crates.example.org:443\r\n\r\n",
    )
    .unwrap();
    assert_eq!(intent.kind, ProxyKind::Connect);
    assert_eq!(intent.host, "crates.example.org");
    assert_eq!(intent.port, 443);
    for request in [
        b"CONNECT crates.example.org:80 HTTP/1.1\r\nHost: crates.example.org:80\r\n\r\n".as_slice(),
        b"CONNECT crates.example.org HTTP/1.1\r\nHost: crates.example.org\r\n\r\n".as_slice(),
        b"CONNECT crates.example.org:0443 HTTP/1.1\r\nHost: crates.example.org:0443\r\n\r\n"
            .as_slice(),
        b"CONNECT crates.example.org:443 HTTP/1.0\r\nHost: crates.example.org:443\r\n\r\n"
            .as_slice(),
        b"CONNECT crates.example.org:443 HTTP/1.1\r\n\r\n".as_slice(),
        b"CONNECT 127.0.0.1:443 HTTP/1.1\r\nHost: 127.0.0.1:443\r\n\r\n".as_slice(),
        b"CONNECT [::1]:443 HTTP/1.1\r\nHost: [::1]:443\r\n\r\n".as_slice(),
        b"CONNECT user@host.example:443 HTTP/1.1\r\n\r\n".as_slice(),
    ] {
        assert!(parse_proxy_request(request).is_err());
    }
}

#[test]
fn proxy_parser_rejects_credentials_bodies_smuggling_and_proxy_chaining() {
    let requests = [
        b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nProxy-Authorization: Basic abc\r\n\r\n".as_slice(),
        b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nContent-Length: 0\r\n\r\n".as_slice(),
        b"POST http://example.com/ HTTP/1.1\r\nHost: example.com\r\nContent-Length: 1\r\n\r\n".as_slice(),
        b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(),
        b"GET http://example.com/ HTTP/1.1\r\nHost: other.example\r\n\r\n".as_slice(),
        b"GET http://user:pass@example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://2130706433/ HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".as_slice(),
        b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nConnection: keep-alive, X-Hop\r\nX-Hop: secret\r\n\r\n".as_slice(),
        b"GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nProxy-Connection: X-Hop\r\nX-Hop: secret\r\n\r\n".as_slice(),
        b"GET /path HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET https://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://%65xample.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://Example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com:00080/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com\\@127.0.0.1/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com/path\nX-Evil:yes HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com/path\rX-Evil:yes HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com/path\tbad HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com/path\x0bbad HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com/path\x0cbad HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com/path\x7fbad HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com//ambiguous HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET  http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n".as_slice(),
        b"GET http://example.com/ HTTP/1.1\nHost: example.com\n\n".as_slice(),
    ];
    for request in requests {
        assert!(
            parse_proxy_request(request).is_err(),
            "{:?}",
            String::from_utf8_lossy(request)
        );
    }
}

#[test]
fn audit_log_size_limit_fails_closed() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = directory.path().join("audit.jsonl");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_AUDIT_BYTES).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(AuditLog::open(&path, "runtime-1".to_string()).is_err());
}

#[tokio::test]
async fn live_uds_broker_denies_and_redacts_malformed_identity() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = directory.path().join("broker.sock");
    let audit_path = directory.path().join("audit.jsonl");
    let cancellation = CancellationToken::new();
    let broker_cancellation = cancellation.clone();
    let broker_socket = socket_path.clone();
    let broker_audit = audit_path.clone();
    let task = tokio::spawn(async move {
        run_broker(
            BrokerConfig {
                socket_path: broker_socket,
                audit_path: broker_audit,
                runtime_id: "runtime-1".to_string(),
                capability: test_capability(),
                expected_peer_uid: rustix::process::geteuid().as_raw(),
                expected_peer_gid: rustix::process::getegid().as_raw(),
                derive_peer_credentials: false,
                expected_peer_pid: std::process::id(),
                selinux_labels: None,
                allow_label_disabled_peer: false,
                max_connections: 2,
            },
            broker_cancellation,
        )
        .await
    });
    for _ in 0..100 {
        if socket_path.exists() {
            break;
        }
        assert!(!task.is_finished(), "broker exited before binding");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let socket_metadata = std::fs::symlink_metadata(&socket_path).unwrap();
    assert!(socket_metadata.file_type().is_socket());
    assert_eq!(socket_metadata.uid(), rustix::process::geteuid().as_raw());
    assert_eq!(socket_metadata.nlink(), 1);
    assert_eq!(socket_metadata.permissions().mode() & 0o7777, 0o600);
    let mut stream = UnixStream::connect(&socket_path).await.unwrap();
    write_frame(
        &mut stream,
        &BrokerRequest {
            version: BROKER_PROTOCOL_VERSION,
            runtime_id: "runtime-1".to_string(),
            capability: test_capability(),
            kind: BrokerRequestKind::Proxy,
            proxy_head_base64: Some(base64::engine::general_purpose::STANDARD.encode(
                b"CONNECT user:password@example.com/private?token=secret HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
            )),
            buffered_after_head: false,
        },
    )
    .await
    .unwrap();
    let response: BrokerResponse = read_frame(&mut stream).await.unwrap();
    assert!(!response.allowed);

    let mut private_stream = UnixStream::connect(&socket_path).await.unwrap();
    let mut wrong_capability = proxy_request(
        "GET http://pypi.org/simple?token=QUERYSECRET HTTP/1.1\r\nHost: pypi.org\r\nAuthorization: Bearer HEADERSECRET\r\n\r\n",
    );
    wrong_capability.capability = "cd".repeat(CAPABILITY_BYTES);
    write_frame(&mut private_stream, &wrong_capability)
        .await
        .unwrap();
    let private_response: BrokerResponse = read_frame(&mut private_stream).await.unwrap();
    assert!(!private_response.allowed);

    cancellation.cancel();
    task.await.unwrap().unwrap();

    let audit = std::fs::read_to_string(&audit_path).unwrap();
    assert!(!audit.contains("secret"));
    assert!(!audit.contains("password"));
    assert!(!audit.contains("QUERYSECRET"));
    assert!(!audit.contains("HEADERSECRET"));
    assert!(!audit.contains("/simple"));
    let records = audit
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 3);
    let request_records = records
        .iter()
        .filter(|record| {
            record["request_id"]
                .as_str()
                .is_some_and(|request_id| request_id.starts_with("egress-"))
        })
        .collect::<Vec<_>>();
    assert_eq!(request_records.len(), 2);
    assert!(request_records[0]["destination"].is_null());
    assert_eq!(request_records[1]["destination"], "pypi.org");
    assert_eq!(records[2]["request_id"], "broker");
    assert_eq!(records[2]["code"], "broker_shutdown");
    let metadata = std::fs::metadata(audit_path).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
}

#[tokio::test]
async fn live_uds_broker_rejects_same_uid_wrong_pid_before_framing() {
    use std::os::unix::fs::PermissionsExt;

    if !crate::platform::binary_on_path("python3") {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket_path = directory.path().join("broker.sock");
    let audit_path = directory.path().join("audit.jsonl");
    let cancellation = CancellationToken::new();
    let broker_cancellation = cancellation.clone();
    let broker_socket = socket_path.clone();
    let broker_audit = audit_path.clone();
    let task = tokio::spawn(async move {
        run_broker(
            BrokerConfig {
                socket_path: broker_socket,
                audit_path: broker_audit,
                runtime_id: "runtime-1".to_string(),
                capability: test_capability(),
                expected_peer_uid: rustix::process::geteuid().as_raw(),
                expected_peer_gid: rustix::process::getegid().as_raw(),
                derive_peer_credentials: false,
                expected_peer_pid: std::process::id(),
                selinux_labels: None,
                allow_label_disabled_peer: false,
                max_connections: 2,
            },
            broker_cancellation,
        )
        .await
    });
    for _ in 0..100 {
        if socket_path.exists() {
            break;
        }
        assert!(!task.is_finished(), "broker exited before binding");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let status = timeout(
        Duration::from_secs(5),
        Command::new("python3")
            .arg("-c")
            .arg(
                "import socket, sys\ns = socket.socket(socket.AF_UNIX)\ns.connect(sys.argv[1])\ntry:\n    s.sendall(b'not-a-frame')\n    s.recv(1)\nexcept OSError:\n    pass\n",
            )
            .arg(&socket_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(status.success());
    cancellation.cancel();
    task.await.unwrap().unwrap();
    let audit = std::fs::read_to_string(audit_path).unwrap();
    let records = audit
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    let record = records
        .iter()
        .find(|record| record["code"] == "peer_credentials")
        .unwrap();
    assert!(record["destination"].is_null());
    assert!(
        records.iter().any(|record| {
            record["request_id"] == "broker" && record["code"] == "broker_shutdown"
        })
    );
}

#[tokio::test]
async fn loopback_adapter_uses_typed_uds_and_rewrites_plain_http() {
    let directory = tempfile::tempdir().unwrap();
    let broker_path = directory.path().join("fake-broker.sock");
    let broker_listener = UnixListener::bind(&broker_path).unwrap();
    let fake_broker = tokio::spawn(async move {
        let (mut stream, _) = broker_listener.accept().await.unwrap();
        let request: BrokerRequest = read_frame(&mut stream).await.unwrap();
        assert_eq!(request.runtime_id, "runtime-1");
        assert_eq!(request.capability, test_capability());
        assert_eq!(request.kind, BrokerRequestKind::Proxy);
        assert!(!request.buffered_after_head);
        let head = decode_proxy_head(&request).unwrap();
        let intent = parse_proxy_request(&head).unwrap();
        assert_eq!(intent.kind, ProxyKind::Http);
        assert_eq!(intent.host, "packages.example.com");
        let rewritten = String::from_utf8(intent.rewritten_head.unwrap()).unwrap();
        assert!(rewritten.starts_with("GET /index?q=rust HTTP/1.1\r\n"));
        assert!(!rewritten.to_ascii_lowercase().contains("proxy-connection"));
        write_frame(
            &mut stream,
            &BrokerResponse {
                version: BROKER_PROTOCOL_VERSION,
                allowed: true,
                code: "connected".to_string(),
                proxy_kind: Some(ProxyKind::Http),
            },
        )
        .await
        .unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        stream.shutdown().await.unwrap();
        let mut unexpected = Vec::new();
        stream.read_to_end(&mut unexpected).await.unwrap();
        assert!(
            unexpected.is_empty(),
            "delayed HTTP bytes crossed the adapter"
        );
    });

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let handler_broker_path = broker_path.clone();
    let handler = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        handle_adapter_connection(
            stream,
            &handler_broker_path,
            "runtime-1",
            &test_capability(),
            CancellationToken::new(),
        )
        .await
    });
    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(
            b"GET http://packages.example.com/index?q=rust HTTP/1.1\r\nHost: packages.example.com\r\nProxy-Connection: keep-alive\r\n\r\n",
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    client
        .write_all(b"GET http://other.example.com/ HTTP/1.1\r\nHost: other.example.com\r\n\r\n")
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(response.ends_with(b"\r\n\r\nOK"));
    handler.await.unwrap().unwrap();
    fake_broker.await.unwrap();
}

#[tokio::test]
async fn framing_roundtrips_and_rejects_oversized_lengths() {
    let request =
        proxy_request("CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n");
    let (mut left, mut right) = duplex(16 * 1024);
    let written = tokio::spawn(async move { write_frame(&mut left, &request).await });
    let decoded: BrokerRequest = read_frame(&mut right).await.unwrap();
    written.await.unwrap().unwrap();
    assert!(
        String::from_utf8(decode_proxy_head(&decoded).unwrap())
            .unwrap()
            .contains("example.com:443")
    );

    let payload = serde_json::to_vec(&proxy_request(
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
    ))
    .unwrap();
    let mut frame = (payload.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&payload);
    let (mut left, mut right) = duplex(frame.len() + 1);
    let bytewise = frame.clone();
    let written = tokio::spawn(async move {
        for byte in bytewise {
            left.write_all(&[byte]).await.unwrap();
        }
    });
    let _: BrokerRequest = read_frame(&mut right).await.unwrap();
    written.await.unwrap();

    for offset in 0..frame.len() {
        let (mut left, mut right) = duplex(frame.len() + 1);
        left.write_all(&frame[..offset]).await.unwrap();
        left.shutdown().await.unwrap();
        assert!(
            read_frame::<_, BrokerRequest>(&mut right).await.is_err(),
            "{offset}"
        );
    }

    let mut maximum_payload = payload.clone();
    maximum_payload.resize(MAX_FRAME_BYTES, b' ');
    let (mut left, mut right) = duplex(MAX_FRAME_BYTES + 4);
    let maximum = tokio::spawn(async move {
        left.write_u32(MAX_FRAME_BYTES as u32).await.unwrap();
        left.write_all(&maximum_payload).await.unwrap();
    });
    let _: BrokerRequest = read_frame(&mut right).await.unwrap();
    maximum.await.unwrap();

    for invalid in [
        br#"{"version":1,"version":1}"#.as_slice(),
        br#"{"version":1,"unknown":true}"#.as_slice(),
    ] {
        let (mut left, mut right) = duplex(256);
        left.write_u32(invalid.len() as u32).await.unwrap();
        left.write_all(invalid).await.unwrap();
        assert!(read_frame::<_, BrokerRequest>(&mut right).await.is_err());
    }

    let (mut left, mut right) = duplex(64);
    left.write_u32(0).await.unwrap();
    assert!(read_frame::<_, BrokerRequest>(&mut right).await.is_err());

    let (mut left, mut right) = duplex(64);
    left.write_u32((MAX_FRAME_BYTES + 1) as u32).await.unwrap();
    assert!(read_frame::<_, BrokerRequest>(&mut right).await.is_err());
}

#[tokio::test]
async fn pre_cancelled_policy_never_polls_outbound_operation() {
    use std::sync::atomic::AtomicUsize;

    let calls = AtomicUsize::new(0);
    let audit_failed = CancellationToken::new();
    let route_changed = CancellationToken::new();
    let shutdown = CancellationToken::new();
    route_changed.cancel();
    let result = run_with_policy(
        async {
            calls.fetch_add(1, Ordering::Relaxed);
        },
        &audit_failed,
        &route_changed,
        &shutdown,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    let audit_calls = AtomicUsize::new(0);
    let failed_audit = CancellationToken::new();
    failed_audit.cancel();
    let result = run_with_policy(
        async {
            audit_calls.fetch_add(1, Ordering::Relaxed);
        },
        &failed_audit,
        &CancellationToken::new(),
        &CancellationToken::new(),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(audit_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn relay_counter_preserves_successful_partial_write_before_error() {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    #[derive(Default)]
    struct PartialFailWriter {
        calls: usize,
    }

    impl AsyncWrite for PartialFailWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.calls += 1;
            if self.calls == 1 {
                Poll::Ready(Ok(bytes.len().min(5)))
            } else {
                Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "injected reset",
                )))
            }
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    let counter = AtomicU64::new(0);
    let result = copy_with_idle_counted(
        std::io::Cursor::new(vec![b'x'; 32]),
        PartialFailWriter::default(),
        &counter,
    )
    .await;
    let error = result.unwrap_err();
    assert_eq!(counter.load(Ordering::Relaxed), 5);
    assert_eq!(classify_tunnel_error(&error), "write_error");
}

#[tokio::test]
async fn counted_write_survives_later_shutdown_failure() {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    #[derive(Default)]
    struct ShutdownFailWriter(Vec<u8>);

    impl AsyncWrite for ShutdownFailWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.0.extend_from_slice(bytes);
            Poll::Ready(Ok(bytes.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::Error::other("injected shutdown failure")))
        }
    }

    let mut writer = ShutdownFailWriter::default();
    let counter = AtomicU64::new(0);
    let audit_failed = CancellationToken::new();
    let route_changed = CancellationToken::new();
    let shutdown = CancellationToken::new();
    let reason = AtomicU64::new(0);
    let payload = vec![b'x'; 123];
    write_counted_with_policy(
        &mut writer,
        &payload,
        &counter,
        &audit_failed,
        &route_changed,
        &shutdown,
        &reason,
    )
    .await
    .unwrap();
    assert_eq!(counter.load(Ordering::Relaxed), 123);
    assert!(
        shutdown_upstream_with_policy(
            &mut writer,
            &audit_failed,
            &route_changed,
            &shutdown,
            &reason,
        )
        .await
        .is_err()
    );
    assert_eq!(counter.load(Ordering::Relaxed), 123);
    assert_eq!(writer.0.len(), 123);

    let mut blocked_writer = ShutdownFailWriter::default();
    let blocked_counter = AtomicU64::new(0);
    let blocked_route = CancellationToken::new();
    blocked_route.cancel();
    assert!(
        write_counted_with_policy(
            &mut blocked_writer,
            b"must-not-cross",
            &blocked_counter,
            &audit_failed,
            &blocked_route,
            &shutdown,
            &reason,
        )
        .await
        .is_err()
    );
    assert!(blocked_writer.0.is_empty());
    assert_eq!(blocked_counter.load(Ordering::Relaxed), 0);
}

#[test]
fn relay_errors_have_distinct_audit_codes() {
    assert_eq!(
        classify_tunnel_error("egress tunnel idle timeout"),
        "idle_timeout"
    );
    assert_eq!(
        classify_tunnel_error("egress tunnel byte limit exceeded"),
        "byte_limit"
    );
    assert_eq!(
        classify_tunnel_error("egress tunnel read failed: injected"),
        "read_error"
    );
    assert_eq!(
        classify_tunnel_error("egress tunnel write failed: injected"),
        "write_error"
    );
    assert_eq!(
        classify_tunnel_error("adapter_response_error"),
        "adapter_response_error"
    );
    assert_eq!(classify_tunnel_error("route_changed"), "route_changed");
}

#[tokio::test]
async fn idle_copy_preserves_bytes_and_half_closes() {
    let (mut source, source_peer) = duplex(1024);
    let (destination, mut destination_peer) = duplex(1024);
    let task = tokio::spawn(async move { copy_with_idle(source_peer, destination).await });
    source.write_all(b"hello").await.unwrap();
    source.shutdown().await.unwrap();
    let mut output = Vec::new();
    destination_peer.read_to_end(&mut output).await.unwrap();
    assert_eq!(output, b"hello");
    assert_eq!(task.await.unwrap().unwrap(), 5);
}

#[test]
fn broker_request_rejects_runtime_capability_protocol_and_pipeline_mismatches() {
    let capability = test_capability();
    let mut request =
        proxy_request("GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\n\r\n");
    assert!(validate_broker_request(&request, "runtime-1", &capability).is_ok());
    request.buffered_after_head = true;
    assert_eq!(
        validate_broker_request(&request, "runtime-1", &capability),
        Err("proxy_pipeline".to_string())
    );
    request.buffered_after_head = false;
    request.capability = "cd".repeat(CAPABILITY_BYTES);
    assert_eq!(
        validate_broker_request(&request, "runtime-1", &capability),
        Err("capability".to_string())
    );
    request.capability = capability.clone();
    request.runtime_id = "runtime-2".to_string();
    assert_eq!(
        validate_broker_request(&request, "runtime-1", &capability),
        Err("runtime_mismatch".to_string())
    );
    request.runtime_id = "runtime-1".to_string();
    request.version += 1;
    assert_eq!(
        validate_broker_request(&request, "runtime-1", &capability),
        Err("protocol_version".to_string())
    );
}

#[test]
fn route_fingerprint_ignores_volatile_kernel_counters_only() {
    let route_a = json!({
        "dst": "8.8.8.8",
        "gateway": "192.168.1.1",
        "dev": "bridge0",
        "prefsrc": "192.168.1.5",
        "uid": 1000,
        "cache": ["expires", 10]
    });
    let mut route_b = route_a.clone();
    route_b["cache"] = json!(["expires", 9]);
    assert_eq!(
        stable_route_fingerprint(&route_a).unwrap(),
        stable_route_fingerprint(&route_b).unwrap()
    );
    route_b["gateway"] = json!("192.168.1.2");
    assert_ne!(
        stable_route_fingerprint(&route_a).unwrap(),
        stable_route_fingerprint(&route_b).unwrap()
    );
    route_b["gateway"] = route_a["gateway"].clone();
    route_b["encap"] = json!({"type":"ip","id":7});
    assert_ne!(
        stable_route_fingerprint(&route_a).unwrap(),
        stable_route_fingerprint(&route_b).unwrap()
    );

    let link_a = json!({
        "ifindex": 4,
        "ifname": "bridge0",
        "flags": ["BROADCAST", "UP", "LOWER_UP"],
        "mtu": 1500,
        "operstate": "UP",
        "link_type": "ether",
        "address": "00:11:22:33:44:55",
        "linkinfo": {
            "info_kind": "bridge",
            "info_data": {"gc_timer": 100.0, "fdb_n_learned": 10}
        }
    });
    let mut link_b = link_a.clone();
    link_b["linkinfo"]["info_data"]["gc_timer"] = json!(99.0);
    assert_eq!(
        stable_link_fingerprint(&link_a).unwrap(),
        stable_link_fingerprint(&link_b).unwrap()
    );
    link_b["address"] = json!("00:11:22:33:44:66");
    assert_ne!(
        stable_link_fingerprint(&link_a).unwrap(),
        stable_link_fingerprint(&link_b).unwrap()
    );
}

#[test]
fn cold_bridge_state_is_retryable_but_ambiguity_is_denied() {
    assert!(matches!(
        classify_gateway_neighbor(&json!([]), "br0").unwrap(),
        BridgeLookup::Cold(_)
    ));
    for state in ["INCOMPLETE", "FAILED"] {
        assert!(matches!(
            classify_gateway_neighbor(&json!([{"dev":"br0","state":[state]}]), "br0").unwrap(),
            BridgeLookup::Cold(_)
        ));
    }
    assert_eq!(
        classify_gateway_neighbor(
            &json!([{
                "dev":"br0",
                "state":["STALE"],
                "lladdr":"00:11:22:33:44:55"
            }]),
            "br0"
        )
        .unwrap(),
        BridgeLookup::Ready("00:11:22:33:44:55".to_string())
    );
    assert!(
        classify_gateway_neighbor(
            &json!([
                {"dev":"br0","state":["STALE"],"lladdr":"00:11:22:33:44:55"},
                {"dev":"br0","state":["STALE"],"lladdr":"00:11:22:33:44:66"}
            ]),
            "br0"
        )
        .is_err()
    );
    assert!(
        classify_gateway_neighbor(
            &json!([{"dev":"other0","state":["STALE"],"lladdr":"00:11:22:33:44:55"}]),
            "br0"
        )
        .is_err()
    );

    assert!(matches!(
        classify_bridge_member(&json!([]), "br0", "00:11:22:33:44:55").unwrap(),
        BridgeLookup::Cold(_)
    ));
    assert_eq!(
        classify_bridge_member(
            &json!([{
                "mac":"00:11:22:33:44:55",
                "master":"br0",
                "ifname":"eth0"
            }]),
            "br0",
            "00:11:22:33:44:55"
        )
        .unwrap(),
        BridgeLookup::Ready("eth0".to_string())
    );
    assert!(
        classify_bridge_member(
            &json!([
                {"mac":"00:11:22:33:44:55","master":"br0","ifname":"eth0"},
                {"mac":"00:11:22:33:44:55","master":"br0","ifname":"eth1"}
            ]),
            "br0",
            "00:11:22:33:44:55"
        )
        .is_err()
    );
}

#[test]
fn route_get_requires_exact_unicast_destination_and_interface() {
    let address = "8.8.8.8".parse().unwrap();
    assert_eq!(
        validate_route_get(
            &json!([{"dst":"8.8.8.8","gateway":"192.168.1.1","dev":"eth0"}]),
            address,
        )
        .unwrap(),
        "eth0"
    );
    for route in [
        json!([]),
        json!([{"dst":"8.8.4.4","dev":"eth0"}]),
        json!([{"dst":"8.8.8.8","dev":"eth0"}]),
        json!([{"dst":"8.8.8.8","gateway":"192.168.1.1","dev":"eth0","table":51820}]),
        json!([{"dst":"8.8.8.8","gateway":"192.168.1.1","dev":"eth0","nexthops":[]}]),
        json!([{"dst":"8.8.8.8","gateway":"192.168.1.1","dev":"eth0","encap":{"type":"seg6","mode":"encap"}}]),
        json!([{"dst":"8.8.8.8","gateway":"192.168.1.1","dev":"eth0","nhid":42}]),
        json!([{"type":"local","dst":"8.8.8.8","dev":"lo"}]),
        json!([{"dst":"8.8.8.8"}]),
        json!([
            {"dst":"8.8.8.8","dev":"eth0"},
            {"dst":"8.8.8.8","dev":"eth1"}
        ]),
    ] {
        assert!(validate_route_get(&route, address).is_err(), "{route}");
    }
}

#[test]
fn renamed_virtual_link_kinds_fail_without_name_heuristics() {
    for kind in [
        "veth", "macvlan", "ipvlan", "dummy", "tun", "tap", "bond", "vlan",
    ] {
        let link = json!({
            "ifname": "safe0",
            "link_type": "ether",
            "linkinfo": {"info_kind": kind}
        });
        assert!(
            validate_physical_link(&link, "safe0", None).is_err(),
            "{kind}"
        );
    }
}

#[test]
fn tunnel_interface_names_fail_closed() {
    for device in [
        "lo",
        "tun0",
        "tap1",
        "wg0",
        "tailscale0",
        "nordlynx",
        "corp-vpn",
        "ppp0",
        "bad/name",
    ] {
        assert!(validate_egress_interface(device).is_err(), "{device}");
    }
}

#[test]
fn unix_socket_path_limit_is_checked_before_bind() {
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let maximum = address.sun_path.len();
    let accepted = PathBuf::from(format!("/{}", "a".repeat(maximum - 2)));
    let rejected = PathBuf::from(format!("/{}", "a".repeat(maximum - 1)));

    assert_eq!(accepted.as_os_str().len(), maximum - 1);
    assert_eq!(rejected.as_os_str().len(), maximum);
    validate_unix_socket_path_length(&accepted).unwrap();
    assert!(validate_unix_socket_path_length(&rejected).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preexisting_shutdown_does_not_start_route_verification() {
    let audit_failed = CancellationToken::new();
    let route_changed = CancellationToken::new();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_for_task = started.clone();

    let error = run_route_verification_with_policy(
        move || {
            started_for_task.store(true, Ordering::SeqCst);
            async { Err("pre-cancelled verification unexpectedly ran".to_string()) }
        },
        &audit_failed,
        &route_changed,
        &shutdown,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error,
        RouteVerificationFailure::Policy("broker connection cancelled")
    );
    assert!(!started.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_route_policy_future_aborts_verification_task() {
    struct DropSignal(std::sync::Arc<std::sync::atomic::AtomicBool>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dropped_for_task = dropped.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let outer = tokio::spawn(async move {
        let audit_failed = CancellationToken::new();
        let route_changed = CancellationToken::new();
        let shutdown = CancellationToken::new();
        run_route_verification_with_policy(
            move || async move {
                let _drop_signal = DropSignal(dropped_for_task);
                let _ = started_tx.send(());
                futures_util::future::pending::<()>().await;
                Err("unreachable route verification".to_string())
            },
            &audit_failed,
            &route_changed,
            &shutdown,
        )
        .await
    });
    started_rx.await.unwrap();
    outer.abort();
    assert!(outer.await.unwrap_err().is_cancelled());
    timeout(Duration::from_secs(2), async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("route verification task detached after its parent was dropped");
}

#[tokio::test]
#[ignore = "depends on the host route and interface inventory"]
async fn real_host_route_policy_preflight() {
    let routes = RouteSnapshot::load().await.unwrap();
    let address = "1.1.1.1".parse().unwrap();
    assert_eq!(
        validate_dns_answers([address], &routes).unwrap(),
        vec![address]
    );
    verify_kernel_route(address).await.unwrap();
}

#[tokio::test]
async fn derived_peer_credentials_are_pinned_and_rechecked() {
    let verifier = PeerVerifier::new(0, 0, std::process::id(), true, None, false).unwrap();
    let (peer, _other) = UnixStream::pair().unwrap();
    verifier.verify(&peer).unwrap();
    assert_eq!(verifier.uid, rustix::process::geteuid().as_raw());
    assert_eq!(verifier.gid, rustix::process::getegid().as_raw());
}

#[test]
fn peer_identity_requires_exact_numeric_address_and_port() {
    fn matches(selected: SocketAddr, peer: SocketAddr) -> bool {
        peer.ip() == selected.ip() && peer.port() == selected.port()
    }
    assert!(matches(
        "8.8.8.8:443".parse().unwrap(),
        "8.8.8.8:443".parse().unwrap()
    ));
    assert!(!matches(
        "8.8.8.8:443".parse().unwrap(),
        "8.8.4.4:443".parse().unwrap()
    ));
    assert!(!matches(
        "8.8.8.8:443".parse().unwrap(),
        "8.8.8.8:80".parse().unwrap()
    ));
}
