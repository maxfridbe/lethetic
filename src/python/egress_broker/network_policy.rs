use super::peer::read_small_text_file;
use serde_json::Value;
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep, timeout};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_DNS_ANSWERS: usize = 32;
const MAX_ROUTE_RECORDS: usize = 4096;
const MAX_ROUTE_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_ROUTE_ERROR_BYTES: usize = 64 * 1024;
const ROUTE_TIMEOUT: Duration = Duration::from_secs(5);
const GATEWAY_RESOLUTION_TIMEOUT: Duration = Duration::from_secs(8);
const GATEWAY_PROBE_PORT: u16 = 9;

static GATEWAY_PROBE_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IpPrefix {
    network: IpAddr,
    prefix: u8,
}

impl IpPrefix {
    pub(super) fn parse(value: &str, family: AddressFamily) -> Result<Self, String> {
        let (address, prefix) = match value.split_once('/') {
            Some((address, prefix)) => {
                let prefix = prefix
                    .parse::<u8>()
                    .map_err(|_| format!("invalid route prefix '{value}'"))?;
                (address, prefix)
            }
            None => (value, family.bits()),
        };
        let network = address
            .parse::<IpAddr>()
            .map_err(|_| format!("invalid route address '{value}'"))?;
        if !family.matches(network) || prefix > family.bits() {
            return Err(format!("route family mismatch for '{value}'"));
        }
        Ok(Self { network, prefix })
    }

    fn contains(self, address: IpAddr) -> bool {
        match (self.network, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                (u32::from(network) & mask) == (u32::from(address) & mask)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                (u128::from(network) & mask) == (u128::from(address) & mask)
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AddressFamily {
    V4,
    V6,
}

impl AddressFamily {
    fn bits(self) -> u8 {
        match self {
            Self::V4 => 32,
            Self::V6 => 128,
        }
    }

    fn matches(self, address: IpAddr) -> bool {
        matches!(
            (self, address),
            (Self::V4, IpAddr::V4(_)) | (Self::V6, IpAddr::V6(_))
        )
    }
}

#[derive(Default, Debug, Clone)]
pub(super) struct RouteSnapshot {
    pub(super) nondefault: Vec<IpPrefix>,
    pub(super) local_addresses: HashSet<IpAddr>,
    pub(super) has_default_v4: bool,
    pub(super) has_default_v6: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RouteDecision {
    Allowed,
    NoDefaultRoute,
    LocalOrSpecificRoute,
}

impl RouteSnapshot {
    pub(super) async fn load() -> Result<Self, String> {
        if !cfg!(target_os = "linux") {
            return Err("host route classification is supported only on Linux".to_string());
        }
        let executable = trusted_ip_executable()?;
        let mut snapshot = Self::default();
        for (family, flag) in [(AddressFamily::V4, "-4"), (AddressFamily::V6, "-6")] {
            let routes =
                run_ip_json(&executable, &["-j", flag, "route", "show", "table", "all"]).await?;
            snapshot.parse_routes(&routes, family)?;
        }
        let addresses = run_ip_json(&executable, &["-j", "address", "show"]).await?;
        snapshot.parse_addresses(&addresses)?;
        if !snapshot.has_default_v4 && !snapshot.has_default_v6 {
            return Err("host has no auditable default IP route".to_string());
        }
        if snapshot.local_addresses.is_empty() {
            return Err("host interface address inventory is empty".to_string());
        }
        Ok(snapshot)
    }

    pub(super) fn parse_routes(
        &mut self,
        value: &Value,
        family: AddressFamily,
    ) -> Result<(), String> {
        let routes = value
            .as_array()
            .ok_or_else(|| "ip route JSON is not an array".to_string())?;
        if routes.len() > MAX_ROUTE_RECORDS {
            return Err("host route record count exceeds the safety limit".to_string());
        }
        for route in routes {
            reject_unsupported_route_features(route)?;
            let route_type = route
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unicast");
            let destination = route
                .get("dst")
                .and_then(Value::as_str)
                .unwrap_or("default");
            let prefix = if destination == "default" {
                IpPrefix {
                    network: match family {
                        AddressFamily::V4 => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                        AddressFamily::V6 => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
                    },
                    prefix: 0,
                }
            } else {
                IpPrefix::parse(destination, family)?
            };
            if prefix.prefix == 0 {
                if route_type == "unicast" {
                    if route.get("nexthops").is_some() {
                        return Err("ECMP default routes are not allowed".to_string());
                    }
                    let device = route
                        .get("dev")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "default route has no interface".to_string())?;
                    validate_interface_name_policy(device)?;
                    if route
                        .get("scope")
                        .and_then(Value::as_str)
                        .is_some_and(|scope| !matches!(scope, "global" | "universe"))
                    {
                        return Err("default route is not global scope".to_string());
                    }
                    match family {
                        AddressFamily::V4 => self.has_default_v4 = true,
                        AddressFamily::V6 => self.has_default_v6 = true,
                    }
                }
            } else {
                self.nondefault.push(prefix);
            }
        }
        Ok(())
    }

    pub(super) fn parse_addresses(&mut self, value: &Value) -> Result<(), String> {
        let interfaces = value
            .as_array()
            .ok_or_else(|| "ip address JSON is not an array".to_string())?;
        if interfaces.len() > MAX_ROUTE_RECORDS {
            return Err("host interface count exceeds the safety limit".to_string());
        }
        for interface in interfaces {
            let Some(addresses) = interface.get("addr_info").and_then(Value::as_array) else {
                continue;
            };
            if addresses.len() > 256 {
                return Err("host interface address count exceeds the safety limit".to_string());
            }
            for address in addresses {
                if let Some(local) = address.get("local").and_then(Value::as_str) {
                    let address = local
                        .split_once('%')
                        .map(|(address, _)| address)
                        .unwrap_or(local)
                        .parse::<IpAddr>()
                        .map_err(|_| format!("invalid host interface address '{local}'"))?;
                    self.local_addresses.insert(normalize_ip(address));
                }
            }
        }
        Ok(())
    }

    pub(super) fn classify(&self, address: IpAddr) -> RouteDecision {
        let address = normalize_ip(address);
        if self.local_addresses.contains(&address)
            || self
                .nondefault
                .iter()
                .any(|prefix| prefix.contains(address))
        {
            return RouteDecision::LocalOrSpecificRoute;
        }
        match address {
            IpAddr::V4(_) if self.has_default_v4 => RouteDecision::Allowed,
            IpAddr::V6(_) if self.has_default_v6 => RouteDecision::Allowed,
            _ => RouteDecision::NoDefaultRoute,
        }
    }
}

fn reject_unsupported_route_features(route: &Value) -> Result<(), String> {
    for field in ["encap", "nhid"] {
        if route.get(field).is_some() {
            return Err(format!(
                "host route uses unsupported kernel forwarding field '{field}'"
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn trusted_system_owner_uid() -> Result<u32, String> {
    if rustix::process::geteuid().as_raw() != 0 {
        return Ok(0);
    }
    let mapping = read_small_text_file(Path::new("/proc/self/uid_map"), 16 * 1024)?;
    let mut rootless_identity_map = false;
    let mut host_root_mapped = false;
    for line in mapping.lines() {
        let values = line
            .split_ascii_whitespace()
            .map(|value| value.parse::<u64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "current user namespace UID map is malformed".to_string())?;
        if values.len() != 3 || values[2] == 0 {
            return Err("current user namespace UID map is malformed".to_string());
        }
        let namespace_start = values[0];
        let host_start = values[1];
        let length = values[2];
        rootless_identity_map |= namespace_start == 0 && host_start > 0 && length == 1;
        host_root_mapped |= host_start == 0;
    }
    if !rootless_identity_map || host_root_mapped {
        return Ok(0);
    }
    read_small_text_file(Path::new("/proc/sys/kernel/overflowuid"), 128)?
        .trim()
        .parse::<u32>()
        .map_err(|_| "kernel overflow UID is malformed".to_string())
}

fn trusted_ip_executable() -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let expected_uid = trusted_system_owner_uid()?;
        for candidate in ["/usr/sbin/ip", "/usr/bin/ip", "/sbin/ip", "/bin/ip"] {
            let path = Path::new(candidate);
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            let metadata = std::fs::metadata(&canonical)
                .map_err(|error| format!("could not inspect ip executable: {error}"))?;
            if !metadata.is_file()
                || metadata.uid() != expected_uid
                || metadata.permissions().mode() & 0o022 != 0
            {
                continue;
            }
            return Ok(canonical);
        }
        Err("trusted root-owned ip executable is unavailable".to_string())
    }
    #[cfg(not(unix))]
    {
        Err("route inspection requires Unix".to_string())
    }
}

fn trusted_bridge_executable() -> Result<PathBuf, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let expected_uid = trusted_system_owner_uid()?;
        for candidate in [
            "/usr/sbin/bridge",
            "/usr/bin/bridge",
            "/sbin/bridge",
            "/bin/bridge",
        ] {
            let path = Path::new(candidate);
            let Ok(canonical) = path.canonicalize() else {
                continue;
            };
            let metadata = std::fs::metadata(&canonical)
                .map_err(|error| format!("could not inspect bridge executable: {error}"))?;
            if metadata.is_file()
                && metadata.uid() == expected_uid
                && metadata.permissions().mode() & 0o022 == 0
            {
                return Ok(canonical);
            }
        }
        Err("trusted root-owned bridge executable is unavailable".to_string())
    }
    #[cfg(not(unix))]
    {
        Err("bridge inspection requires Unix".to_string())
    }
}

async fn run_ip_json(executable: &Path, args: &[&str]) -> Result<Value, String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("host route inspection failed: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "host route inspection has no stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "host route inspection has no stderr".to_string())?;
    let outcome = timeout(ROUTE_TIMEOUT, async {
        let read_stdout = read_bounded(stdout, MAX_ROUTE_OUTPUT_BYTES, "route output");
        let read_stderr = read_bounded(stderr, MAX_ROUTE_ERROR_BYTES, "route diagnostics");
        let wait = async {
            child
                .wait()
                .await
                .map_err(|error| format!("host route inspection wait failed: {error}"))
        };
        tokio::try_join!(read_stdout, read_stderr, wait)
    })
    .await;
    let (stdout, stderr, status) = match outcome {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err(error);
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return Err("host route inspection timed out".to_string());
        }
    };
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(format!(
            "host route inspection exited {status}: {}",
            stderr.trim()
        ));
    }
    serde_json::from_slice(&stdout).map_err(|error| format!("invalid host route JSON: {error}"))
}

async fn read_bounded<R>(mut reader: R, maximum: usize, label: &str) -> Result<Vec<u8>, String>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .await
            .map_err(|error| format!("could not read {label}: {error}"))?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > maximum {
            return Err(format!("{label} exceeds the safety limit"));
        }
        output.extend_from_slice(&buffer[..read]);
    }
}

#[cfg(target_os = "linux")]
pub(super) struct HostNetworkMonitor {
    socket: tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>,
}

#[cfg(target_os = "linux")]
impl HostNetworkMonitor {
    pub(super) fn bind() -> Result<Self, String> {
        use std::os::fd::FromRawFd;

        // Neighbor/FDB state churn is intentionally not subscribed here. Every
        // outbound connect independently fingerprints the selected gateway and
        // bridge member before and after connecting; global FDB notifications
        // include unrelated LAN/VM entries and would otherwise terminate safe
        // sessions continuously.
        const GROUPS: u32 = 0x0000_0001 // link
            | 0x0000_0010 // IPv4 address
            | 0x0000_0040 // IPv4 route
            | 0x0000_0080 // IPv4 rule
            | 0x0000_0100 // IPv6 address
            | 0x0000_0400 // IPv6 route
            | 0x0004_0000 // IPv6 rule
            | (1_u32 << (libc::RTNLGRP_NEXTHOP - 1)); // nexthop objects
        let raw = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                libc::NETLINK_ROUTE,
            )
        };
        if raw < 0 {
            return Err(format!(
                "could not create route-monitor netlink socket: {}",
                std::io::Error::last_os_error()
            ));
        }
        let socket = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
        let receive_bytes: libc::c_int = 1024 * 1024;
        let receive_result = unsafe {
            libc::setsockopt(
                raw,
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                (&receive_bytes as *const libc::c_int).cast(),
                std::mem::size_of_val(&receive_bytes) as libc::socklen_t,
            )
        };
        if receive_result != 0 {
            return Err(format!(
                "could not size route-monitor receive buffer: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut address: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        address.nl_pid = 0;
        address.nl_groups = GROUPS;
        let bind_result = unsafe {
            libc::bind(
                raw,
                (&address as *const libc::sockaddr_nl).cast(),
                std::mem::size_of_val(&address) as libc::socklen_t,
            )
        };
        if bind_result != 0 {
            return Err(format!(
                "could not subscribe route-monitor netlink socket: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut bound: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        let mut bound_length = std::mem::size_of_val(&bound) as libc::socklen_t;
        let name_result = unsafe {
            libc::getsockname(
                raw,
                (&mut bound as *mut libc::sockaddr_nl).cast(),
                &mut bound_length,
            )
        };
        if name_result != 0
            || bound_length as usize != std::mem::size_of_val(&bound)
            || bound.nl_family != libc::AF_NETLINK as libc::sa_family_t
            || bound.nl_groups & GROUPS != GROUPS
        {
            return Err("route-monitor netlink subscription could not be verified".to_string());
        }
        let socket = tokio::io::unix::AsyncFd::new(socket)
            .map_err(|error| format!("could not register route-monitor socket: {error}"))?;
        Ok(Self { socket })
    }

    pub(super) async fn run(self, shutdown: CancellationToken, changed: CancellationToken) {
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let mut ready = tokio::select! {
                _ = shutdown.cancelled() => return,
                ready = self.socket.readable() => match ready {
                    Ok(ready) => ready,
                    Err(_) => {
                        changed.cancel();
                        return;
                    }
                },
            };
            let result = ready.try_io(|socket| {
                use std::os::fd::AsRawFd;
                let read = unsafe {
                    libc::recv(
                        socket.get_ref().as_raw_fd(),
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                        libc::MSG_DONTWAIT | libc::MSG_TRUNC,
                    )
                };
                if read < 0 {
                    Err(std::io::Error::last_os_error())
                } else if read as usize > buffer.len() {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "route-monitor netlink datagram was truncated",
                    ))
                } else {
                    Ok(read as usize)
                }
            });
            match result {
                Err(_) => continue,
                Ok(Ok(read)) => match netlink_invalidates_policy(&buffer[..read]) {
                    Ok(false) => continue,
                    Ok(true) | Err(_) => {
                        changed.cancel();
                        return;
                    }
                },
                Ok(Err(_)) => {
                    changed.cancel();
                    return;
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn netlink_invalidates_policy(bytes: &[u8]) -> Result<bool, String> {
    const HEADER_BYTES: usize = 16;
    let mut offset = 0_usize;
    if bytes.is_empty() {
        return Err("route-monitor netlink socket reached EOF".to_string());
    }
    while offset < bytes.len() {
        let remaining = bytes.len() - offset;
        if remaining < HEADER_BYTES {
            return Err("route-monitor received a truncated netlink header".to_string());
        }
        let length = u32::from_ne_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .map_err(|_| "route-monitor netlink length is malformed".to_string())?,
        ) as usize;
        if length < HEADER_BYTES || length > remaining {
            return Err("route-monitor netlink message length is invalid".to_string());
        }
        let message_type = u16::from_ne_bytes(
            bytes[offset + 4..offset + 6]
                .try_into()
                .map_err(|_| "route-monitor netlink type is malformed".to_string())?,
        );
        match message_type {
            message_type
                if message_type == libc::NLMSG_NOOP as u16
                    || message_type == libc::NLMSG_DONE as u16 => {}
            message_type if message_type == libc::NLMSG_ERROR as u16 => {
                return Err("route-monitor received a netlink error".to_string());
            }
            libc::RTM_NEWNEIGH | libc::RTM_DELNEIGH => {
                if length < HEADER_BYTES + 12 {
                    return Err("route-monitor neighbor message is truncated".to_string());
                }
                let family = bytes[offset + HEADER_BYTES] as libc::c_int;
                if !matches!(family, libc::AF_INET | libc::AF_INET6 | libc::AF_BRIDGE) {
                    return Err("route-monitor received an unknown neighbor family".to_string());
                }
            }
            _ => return Ok(true),
        }
        let aligned = length
            .checked_add(3)
            .map(|length| length & !3)
            .ok_or_else(|| "route-monitor netlink alignment overflowed".to_string())?;
        offset = offset
            .checked_add(aligned)
            .ok_or_else(|| "route-monitor netlink offset overflowed".to_string())?;
        if offset > bytes.len() {
            return Err("route-monitor netlink padding is truncated".to_string());
        }
    }
    Ok(false)
}

#[cfg(not(target_os = "linux"))]
pub(super) struct HostNetworkMonitor;

#[cfg(not(target_os = "linux"))]
impl HostNetworkMonitor {
    pub(super) fn bind() -> Result<Self, String> {
        Err("host network monitoring requires Linux route netlink".to_string())
    }

    pub(super) async fn run(self, _shutdown: CancellationToken, changed: CancellationToken) {
        changed.cancel();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RouteFingerprint {
    route: Value,
    device: String,
    topology: Value,
}

pub(super) async fn verify_kernel_route(address: IpAddr) -> Result<RouteFingerprint, String> {
    if !is_public_destination(address) {
        return Err("kernel route target is not public".to_string());
    }
    let executable = trusted_ip_executable()?;
    let family = if address.is_ipv4() { "-4" } else { "-6" };
    let target = address.to_string();
    let route = run_ip_json(&executable, &["-j", family, "route", "get", &target]).await?;
    let device = validate_route_get(&route, address)?.to_string();
    validate_egress_interface(&device)?;
    let route = route
        .as_array()
        .and_then(|routes| routes.first())
        .cloned()
        .ok_or_else(|| "kernel route fingerprint is empty".to_string())?;
    let route_fingerprint = stable_route_fingerprint(&route)?;
    let gateway = parse_route_ip(&route, &["gateway"], address, "gateway")?
        .ok_or_else(|| "kernel route has no gateway".to_string())?;
    let source = parse_route_ip(&route, &["prefsrc", "src"], address, "source")?;
    let topology = classify_egress_topology(
        &executable,
        address,
        &device,
        gateway,
        source,
        &route_fingerprint,
    )
    .await?;
    Ok(RouteFingerprint {
        route: route_fingerprint,
        device,
        topology,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RouteVerificationFailure {
    Policy(&'static str),
    Attestation(String),
}

pub(super) async fn run_route_verification_with_policy<F, Fut>(
    verification: F,
    audit_failed: &CancellationToken,
    route_changed: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<RouteFingerprint, RouteVerificationFailure>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<RouteFingerprint, String>> + Send + 'static,
{
    for (cancelled, message) in [
        (audit_failed, "broker audit log failed"),
        (route_changed, "host network policy changed"),
        (shutdown, "broker connection cancelled"),
    ] {
        if cancelled.is_cancelled() {
            return Err(RouteVerificationFailure::Policy(message));
        }
    }

    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(verification());
    tokio::select! {
        biased;
        _ = audit_failed.cancelled() => {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            Err(RouteVerificationFailure::Policy("broker audit log failed"))
        }
        _ = route_changed.cancelled() => {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            Err(RouteVerificationFailure::Policy("host network policy changed"))
        }
        _ = shutdown.cancelled() => {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
            Err(RouteVerificationFailure::Policy("broker connection cancelled"))
        }
        joined = tasks.join_next() => match joined {
            Some(Ok(Ok(fingerprint))) => Ok(fingerprint),
            Some(Ok(Err(error))) => Err(RouteVerificationFailure::Attestation(error)),
            Some(Err(error)) => Err(RouteVerificationFailure::Attestation(format!(
                "kernel route verification task failed: {error}"
            ))),
            None => Err(RouteVerificationFailure::Attestation(
                "kernel route verification task disappeared".to_string(),
            )),
        }
    }
}

pub(super) async fn verify_kernel_route_with_policy(
    address: IpAddr,
    audit_failed: &CancellationToken,
    route_changed: &CancellationToken,
    shutdown: &CancellationToken,
) -> Result<RouteFingerprint, RouteVerificationFailure> {
    // Route/topology attestation has several nested async command probes. Poll it in its
    // own abort-on-drop task set so a large connection future cannot exhaust a Tokio
    // worker stack or leave attestation work detached after connection teardown.
    run_route_verification_with_policy(
        move || verify_kernel_route(address),
        audit_failed,
        route_changed,
        shutdown,
    )
    .await
}

pub(super) fn stable_route_fingerprint(route: &Value) -> Result<Value, String> {
    let object = route
        .as_object()
        .ok_or_else(|| "kernel route fingerprint is not an object".to_string())?;
    let mut stable = serde_json::Map::new();
    for key in [
        "type", "dst", "gateway", "dev", "prefsrc", "src", "table", "scope", "protocol", "metric",
        "mark", "uid", "flags", "encap", "nhid",
    ] {
        if let Some(value) = object.get(key) {
            stable.insert(key.to_string(), value.clone());
        }
    }
    for required in ["dst", "gateway", "dev"] {
        if !stable.contains_key(required) {
            return Err(format!("kernel route fingerprint has no {required}"));
        }
    }
    Ok(Value::Object(stable))
}

fn parse_route_ip(
    route: &Value,
    fields: &[&str],
    selected: IpAddr,
    label: &str,
) -> Result<Option<IpAddr>, String> {
    let Some(raw) = fields
        .iter()
        .find_map(|field| route.get(*field).and_then(Value::as_str))
    else {
        return Ok(None);
    };
    let address = raw
        .split_once('%')
        .map(|(address, _)| address)
        .unwrap_or(raw)
        .parse::<IpAddr>()
        .map(normalize_ip)
        .map_err(|_| format!("kernel route {label} is not numeric"))?;
    if address.is_ipv4() != selected.is_ipv4()
        || address.is_unspecified()
        || address.is_loopback()
        || address.is_multicast()
    {
        return Err(format!("kernel route {label} is invalid"));
    }
    Ok(Some(address))
}

#[derive(Debug)]
enum BridgeTopologyStatus {
    Ready(Value),
    Cold(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BridgeLookup<T> {
    Ready(T),
    Cold(String),
}

pub(super) fn classify_gateway_neighbor(
    neighbors: &Value,
    device: &str,
) -> Result<BridgeLookup<String>, String> {
    let neighbors = neighbors
        .as_array()
        .ok_or_else(|| "gateway neighbor JSON is not an array".to_string())?;
    if neighbors.is_empty() {
        return Ok(BridgeLookup::Cold("gateway neighbor is absent".to_string()));
    }
    if neighbors.len() != 1 {
        return Err("gateway neighbor result is ambiguous".to_string());
    }
    let neighbor = &neighbors[0];
    if neighbor
        .get("dev")
        .and_then(Value::as_str)
        .is_some_and(|reported| reported != device)
    {
        return Err("gateway neighbor uses a different interface".to_string());
    }
    let states = neighbor
        .get("state")
        .and_then(Value::as_array)
        .ok_or_else(|| "gateway neighbor state is malformed".to_string())?;
    if states.is_empty() || states.iter().any(|state| state.as_str().is_none()) {
        return Err("gateway neighbor state is malformed".to_string());
    }
    let usable = states.iter().all(|state| {
        state.as_str().is_some_and(|state| {
            matches!(
                state,
                "REACHABLE" | "STALE" | "DELAY" | "PROBE" | "PERMANENT"
            )
        })
    });
    if !usable {
        let cold = states.iter().all(|state| {
            state
                .as_str()
                .is_some_and(|state| matches!(state, "INCOMPLETE" | "FAILED"))
        });
        if cold {
            return Ok(BridgeLookup::Cold(
                "gateway neighbor is unresolved".to_string(),
            ));
        }
        return Err("gateway neighbor has an unsupported state".to_string());
    }
    let gateway_mac = neighbor
        .get("lladdr")
        .and_then(Value::as_str)
        .ok_or_else(|| "gateway neighbor has no link-layer address".to_string())?;
    if !is_canonical_mac(gateway_mac) {
        return Err("gateway neighbor link-layer address is malformed".to_string());
    }
    Ok(BridgeLookup::Ready(gateway_mac.to_string()))
}

pub(super) fn classify_bridge_member(
    fdb: &Value,
    device: &str,
    gateway_mac: &str,
) -> Result<BridgeLookup<String>, String> {
    let entries = fdb
        .as_array()
        .ok_or_else(|| "bridge FDB JSON is not an array".to_string())?;
    if entries.len() > MAX_ROUTE_RECORDS {
        return Err("bridge FDB record count exceeds the safety limit".to_string());
    }
    let mut members = HashSet::new();
    for entry in entries.iter().filter(|entry| {
        entry.get("mac").and_then(Value::as_str) == Some(gateway_mac)
            && entry.get("master").and_then(Value::as_str) == Some(device)
    }) {
        let member = entry
            .get("ifname")
            .and_then(Value::as_str)
            .ok_or_else(|| "gateway bridge FDB member is malformed".to_string())?;
        members.insert(member);
    }
    if members.is_empty() {
        return Ok(BridgeLookup::Cold(
            "gateway bridge FDB member is absent".to_string(),
        ));
    }
    if members.len() != 1 {
        return Err("gateway bridge egress member is ambiguous".to_string());
    }
    Ok(BridgeLookup::Ready(
        (*members.iter().next().expect("one bridge member")).to_string(),
    ))
}

async fn classify_egress_topology(
    ip: &Path,
    selected_address: IpAddr,
    device: &str,
    gateway: IpAddr,
    source: Option<IpAddr>,
    expected_route: &Value,
) -> Result<Value, String> {
    let selected = load_link_record(ip, device).await?;
    let kind = selected
        .get("linkinfo")
        .and_then(|linkinfo| linkinfo.get("info_kind"))
        .and_then(Value::as_str);
    match kind {
        None => {
            validate_physical_link(&selected, device, None)?;
            json_topology("physical", selected, None, None)
        }
        Some("bridge") => {
            let expected_link = stable_link_fingerprint(&selected)?;
            match inspect_bridge_topology(ip, device, gateway, selected.clone()).await? {
                BridgeTopologyStatus::Ready(topology) => return Ok(topology),
                BridgeTopologyStatus::Cold(_) => {}
            }

            let _probe_guard = GATEWAY_PROBE_LOCK.lock().await;
            let selected = revalidate_static_bridge_route(
                ip,
                selected_address,
                device,
                gateway,
                source,
                expected_route,
                &expected_link,
            )
            .await?;
            match inspect_bridge_topology(ip, device, gateway, selected.clone()).await? {
                BridgeTopologyStatus::Ready(topology) => return Ok(topology),
                BridgeTopologyStatus::Cold(_) => {}
            }

            let source = source.ok_or_else(|| {
                "cold bridge route has no validated source address for neighbor resolution"
                    .to_string()
            })?;
            validate_source_on_interface(ip, device, source).await?;
            let ifindex = selected
                .get("ifindex")
                .and_then(Value::as_u64)
                .and_then(|index| u32::try_from(index).ok())
                .filter(|index| *index != 0)
                .ok_or_else(|| "egress bridge has no valid interface index".to_string())?;
            let _probe = send_gateway_probe(gateway, source, ifindex).await?;
            let deadline = Instant::now() + GATEWAY_RESOLUTION_TIMEOUT;
            let mut attempt = 0_usize;
            let mut last_cold = "gateway neighbor is unresolved".to_string();

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(format!(
                        "gateway neighbor/FDB resolution timed out: {last_cold}"
                    ));
                }
                let status = timeout(
                    remaining,
                    inspect_bridge_topology(ip, device, gateway, selected.clone()),
                )
                .await
                .map_err(|_| format!("gateway neighbor/FDB resolution timed out: {last_cold}"))??;
                match status {
                    BridgeTopologyStatus::Cold(reason) => last_cold = reason,
                    BridgeTopologyStatus::Ready(_) => {
                        let final_selected = revalidate_static_bridge_route(
                            ip,
                            selected_address,
                            device,
                            gateway,
                            Some(source),
                            expected_route,
                            &expected_link,
                        )
                        .await?;
                        match inspect_bridge_topology(ip, device, gateway, final_selected).await? {
                            BridgeTopologyStatus::Ready(topology) => return Ok(topology),
                            BridgeTopologyStatus::Cold(reason) => last_cold = reason,
                        }
                    }
                }
                let delay = match attempt {
                    0 => Duration::from_millis(100),
                    1 => Duration::from_millis(250),
                    _ => Duration::from_millis(500),
                };
                attempt = attempt.saturating_add(1);
                sleep(delay.min(deadline.saturating_duration_since(Instant::now()))).await;
            }
        }
        Some(_) => Err("kernel route selected an unsupported virtual link kind".to_string()),
    }
}

async fn inspect_bridge_topology(
    ip: &Path,
    device: &str,
    gateway: IpAddr,
    selected: Value,
) -> Result<BridgeTopologyStatus, String> {
    let gateway_text = gateway.to_string();
    let neighbors = run_ip_json(
        ip,
        &["-j", "neigh", "show", "to", &gateway_text, "dev", device],
    )
    .await?;
    let gateway_mac = match classify_gateway_neighbor(&neighbors, device)? {
        BridgeLookup::Ready(mac) => mac,
        BridgeLookup::Cold(reason) => return Ok(BridgeTopologyStatus::Cold(reason)),
    };
    let bridge = trusted_bridge_executable()?;
    let fdb = run_ip_json(&bridge, &["-j", "fdb", "show", "br", device]).await?;
    let member = match classify_bridge_member(&fdb, device, &gateway_mac)? {
        BridgeLookup::Ready(member) => member,
        BridgeLookup::Cold(reason) => return Ok(BridgeTopologyStatus::Cold(reason)),
    };
    validate_interface_name_policy(&member)?;
    let member_record = load_link_record(ip, &member).await?;
    validate_physical_link(&member_record, &member, Some(device))?;
    let neighbor_fingerprint = serde_json::json!({
        "dev": device,
        "lladdr": gateway_mac,
    });
    Ok(BridgeTopologyStatus::Ready(json_topology(
        "physical_bridge_gateway",
        selected,
        Some(member_record),
        Some(neighbor_fingerprint),
    )?))
}

async fn revalidate_static_bridge_route(
    ip: &Path,
    selected_address: IpAddr,
    device: &str,
    gateway: IpAddr,
    source: Option<IpAddr>,
    expected_route: &Value,
    expected_link: &Value,
) -> Result<Value, String> {
    let family = if selected_address.is_ipv4() {
        "-4"
    } else {
        "-6"
    };
    let target = selected_address.to_string();
    let routes = run_ip_json(ip, &["-j", family, "route", "get", &target]).await?;
    let current_device = validate_route_get(&routes, selected_address)?;
    if current_device != device {
        return Err("kernel route interface changed during gateway resolution".to_string());
    }
    validate_egress_interface(current_device)?;
    let route = routes
        .as_array()
        .and_then(|routes| routes.first())
        .ok_or_else(|| "kernel route disappeared during gateway resolution".to_string())?;
    if stable_route_fingerprint(route)? != *expected_route {
        return Err("kernel route changed during gateway resolution".to_string());
    }
    if parse_route_ip(route, &["gateway"], selected_address, "gateway")? != Some(gateway)
        || parse_route_ip(route, &["prefsrc", "src"], selected_address, "source")? != source
    {
        return Err("kernel route endpoint changed during gateway resolution".to_string());
    }
    let selected = load_link_record(ip, device).await?;
    if selected
        .get("linkinfo")
        .and_then(|linkinfo| linkinfo.get("info_kind"))
        .and_then(Value::as_str)
        != Some("bridge")
        || stable_link_fingerprint(&selected)? != *expected_link
    {
        return Err("egress bridge changed during gateway resolution".to_string());
    }
    Ok(selected)
}

async fn validate_source_on_interface(
    ip: &Path,
    device: &str,
    source: IpAddr,
) -> Result<(), String> {
    let interfaces = run_ip_json(ip, &["-j", "address", "show", "dev", device]).await?;
    let interfaces = interfaces
        .as_array()
        .ok_or_else(|| "egress source address JSON is not an array".to_string())?;
    if interfaces.len() != 1 {
        return Err("egress source interface result is ambiguous".to_string());
    }
    let matches = interfaces[0]
        .get("addr_info")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|address| address.get("local").and_then(Value::as_str))
        .filter_map(|address| {
            address
                .split_once('%')
                .map(|(address, _)| address)
                .unwrap_or(address)
                .parse::<IpAddr>()
                .ok()
        })
        .map(normalize_ip)
        .filter(|address| *address == source)
        .count();
    if matches != 1 {
        return Err("kernel route source is not uniquely assigned to its bridge".to_string());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
struct GatewayProbeSocket {
    _socket: tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>,
}

#[cfg(target_os = "linux")]
async fn send_gateway_probe(
    gateway: IpAddr,
    source: IpAddr,
    ifindex: u32,
) -> Result<GatewayProbeSocket, String> {
    use std::os::fd::{AsRawFd, FromRawFd};

    if gateway.is_ipv4() != source.is_ipv4() || ifindex == 0 {
        return Err("gateway probe parameters are inconsistent".to_string());
    }
    let family = if gateway.is_ipv4() {
        libc::AF_INET
    } else {
        libc::AF_INET6
    };
    let raw = unsafe {
        libc::socket(
            family,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw < 0 {
        return Err(format!(
            "could not create gateway probe socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw) };
    let hop_limit: libc::c_int = 1;
    let (level, option) = if family == libc::AF_INET {
        (libc::IPPROTO_IP, libc::IP_TTL)
    } else {
        (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS)
    };
    if unsafe {
        libc::setsockopt(
            raw,
            level,
            option,
            (&hop_limit as *const libc::c_int).cast(),
            std::mem::size_of_val(&hop_limit) as libc::socklen_t,
        )
    } != 0
    {
        return Err(format!(
            "could not constrain gateway probe hop limit: {}",
            std::io::Error::last_os_error()
        ));
    }

    #[repr(C)]
    union ControlBuffer {
        _alignment: libc::cmsghdr,
        bytes: [u8; 128],
    }
    let mut control = ControlBuffer { bytes: [0; 128] };
    let mut destination: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let (destination_length, control_data_length, control_level, control_type) =
        match (gateway, source) {
            (IpAddr::V4(gateway), IpAddr::V4(_)) => {
                let address =
                    unsafe { &mut *(&mut destination as *mut _ as *mut libc::sockaddr_in) };
                address.sin_family = libc::AF_INET as libc::sa_family_t;
                address.sin_port = GATEWAY_PROBE_PORT.to_be();
                address.sin_addr = libc::in_addr {
                    s_addr: u32::from_ne_bytes(gateway.octets()),
                };
                (
                    std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                    std::mem::size_of::<libc::in_pktinfo>(),
                    libc::IPPROTO_IP,
                    libc::IP_PKTINFO,
                )
            }
            (IpAddr::V6(gateway), IpAddr::V6(_)) => {
                let address =
                    unsafe { &mut *(&mut destination as *mut _ as *mut libc::sockaddr_in6) };
                address.sin6_family = libc::AF_INET6 as libc::sa_family_t;
                address.sin6_port = GATEWAY_PROBE_PORT.to_be();
                address.sin6_addr = libc::in6_addr {
                    s6_addr: gateway.octets(),
                };
                address.sin6_scope_id = if gateway.is_unicast_link_local() {
                    ifindex
                } else {
                    0
                };
                (
                    std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t,
                    std::mem::size_of::<libc::in6_pktinfo>(),
                    libc::IPPROTO_IPV6,
                    libc::IPV6_PKTINFO,
                )
            }
            _ => return Err("gateway probe address families differ".to_string()),
        };
    let control_length = unsafe { libc::CMSG_SPACE(control_data_length as libc::c_uint) } as usize;
    if control_length > 128 {
        return Err("gateway probe control message is too large".to_string());
    }
    let mut empty = libc::iovec {
        iov_base: std::ptr::null_mut(),
        iov_len: 0,
    };
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_name = (&mut destination as *mut libc::sockaddr_storage).cast();
    message.msg_namelen = destination_length;
    message.msg_iov = &mut empty;
    message.msg_iovlen = 1;
    message.msg_control = unsafe { control.bytes.as_mut_ptr().cast() };
    message.msg_controllen = control_length;
    let header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    if header.is_null() {
        return Err("could not construct gateway probe control message".to_string());
    }
    unsafe {
        (*header).cmsg_level = control_level;
        (*header).cmsg_type = control_type;
        (*header).cmsg_len = libc::CMSG_LEN(control_data_length as libc::c_uint) as usize;
        match source {
            IpAddr::V4(source) => {
                std::ptr::write(
                    libc::CMSG_DATA(header).cast::<libc::in_pktinfo>(),
                    libc::in_pktinfo {
                        ipi_ifindex: i32::try_from(ifindex)
                            .map_err(|_| "gateway probe interface index is too large")?,
                        ipi_spec_dst: libc::in_addr {
                            s_addr: u32::from_ne_bytes(source.octets()),
                        },
                        ipi_addr: libc::in_addr { s_addr: 0 },
                    },
                );
            }
            IpAddr::V6(source) => {
                std::ptr::write(
                    libc::CMSG_DATA(header).cast::<libc::in6_pktinfo>(),
                    libc::in6_pktinfo {
                        ipi6_addr: libc::in6_addr {
                            s6_addr: source.octets(),
                        },
                        ipi6_ifindex: ifindex,
                    },
                );
            }
        }
    }

    let socket = tokio::io::unix::AsyncFd::new(owned)
        .map_err(|error| format!("could not register gateway probe socket: {error}"))?;
    let sent = unsafe {
        libc::sendmsg(
            socket.get_ref().as_raw_fd(),
            &message,
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
        )
    };
    if sent == 0 {
        Ok(GatewayProbeSocket { _socket: socket })
    } else if sent > 0 {
        Err("gateway probe unexpectedly contained payload".to_string())
    } else {
        Err(format!(
            "could not send gateway neighbor probe: {}",
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(not(target_os = "linux"))]
struct GatewayProbeSocket;

#[cfg(not(target_os = "linux"))]
async fn send_gateway_probe(
    _gateway: IpAddr,
    _source: IpAddr,
    _ifindex: u32,
) -> Result<GatewayProbeSocket, String> {
    Err("gateway neighbor probing requires Linux".to_string())
}

async fn load_link_record(ip: &Path, device: &str) -> Result<Value, String> {
    validate_interface_name_policy(device)?;
    let links = run_ip_json(ip, &["-j", "-d", "link", "show", "dev", device]).await?;
    let links = links
        .as_array()
        .ok_or_else(|| "detailed link JSON is not an array".to_string())?;
    if links.len() != 1 {
        return Err("detailed link result is ambiguous".to_string());
    }
    let link = links[0].clone();
    if link.get("ifname").and_then(Value::as_str) != Some(device) {
        return Err("detailed link result has a mismatched interface".to_string());
    }
    Ok(link)
}

pub(super) fn validate_physical_link(
    link: &Value,
    device: &str,
    expected_master: Option<&str>,
) -> Result<(), String> {
    if link
        .get("linkinfo")
        .and_then(|linkinfo| linkinfo.get("info_kind"))
        .and_then(Value::as_str)
        .is_some()
    {
        return Err("egress lower link is virtual".to_string());
    }
    if link.get("master").and_then(Value::as_str) != expected_master {
        return Err("egress lower link has an unexpected master".to_string());
    }
    if !matches!(
        link.get("link_type").and_then(Value::as_str),
        Some("ether" | "infiniband")
    ) {
        return Err("egress lower link has an unsupported hardware type".to_string());
    }
    let sysfs = Path::new("/sys/class/net").join(device);
    if !sysfs.join("device").exists()
        || sysfs.join("tun_flags").exists()
        || !sysfs.join("ifindex").is_file()
        || !sysfs.join("iflink").is_file()
    {
        return Err("egress lower link has no physical device backing".to_string());
    }
    Ok(())
}

fn is_canonical_mac(value: &str) -> bool {
    value.len() == 17
        && value.split(':').count() == 6
        && value.split(':').all(|octet| {
            octet.len() == 2
                && octet
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
}

pub(super) fn stable_link_fingerprint(link: &Value) -> Result<Value, String> {
    let object = link
        .as_object()
        .ok_or_else(|| "link fingerprint is not an object".to_string())?;
    let mut stable = serde_json::Map::new();
    for key in [
        "ifindex",
        "ifname",
        "flags",
        "mtu",
        "operstate",
        "link_type",
        "address",
        "master",
    ] {
        if let Some(value) = object.get(key) {
            stable.insert(key.to_string(), value.clone());
        }
    }
    let kind = object
        .get("linkinfo")
        .and_then(|linkinfo| linkinfo.get("info_kind"))
        .cloned();
    if let Some(kind) = kind {
        stable.insert("info_kind".to_string(), kind);
    }
    for required in ["ifindex", "ifname", "flags", "link_type"] {
        if !stable.contains_key(required) {
            return Err(format!("link fingerprint has no {required}"));
        }
    }
    Ok(Value::Object(stable))
}

fn json_topology(
    kind: &str,
    selected: Value,
    lower: Option<Value>,
    neighbor: Option<Value>,
) -> Result<Value, String> {
    let selected = stable_link_fingerprint(&selected)?;
    let lower = lower.as_ref().map(stable_link_fingerprint).transpose()?;
    Ok(serde_json::json!({
        "kind": kind,
        "selected": selected,
        "lower": lower,
        "neighbor": neighbor,
    }))
}

pub(super) fn validate_route_get<'a>(value: &'a Value, address: IpAddr) -> Result<&'a str, String> {
    let routes = value
        .as_array()
        .ok_or_else(|| "ip route-get JSON is not an array".to_string())?;
    if routes.len() != 1 {
        return Err("ip route-get returned an ambiguous result".to_string());
    }
    let route = &routes[0];
    reject_unsupported_route_features(route)?;
    if route
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|route_type| route_type != "unicast")
    {
        return Err("kernel selected a non-unicast route".to_string());
    }
    if route.get("nexthops").is_some() {
        return Err("kernel selected an ECMP route".to_string());
    }
    if route
        .get("scope")
        .and_then(Value::as_str)
        .is_some_and(|scope| !matches!(scope, "global" | "universe"))
    {
        return Err("kernel selected a non-global route".to_string());
    }
    if let Some(table) = route.get("table") {
        let main_table = table
            .as_str()
            .is_some_and(|table| matches!(table, "main" | "254"))
            || table.as_u64() == Some(254);
        if !main_table {
            return Err("kernel selected a non-main policy route".to_string());
        }
    }
    if route.get("gateway").and_then(Value::as_str).is_none() {
        return Err("kernel selected an on-link public route".to_string());
    }
    let destination = route
        .get("dst")
        .and_then(Value::as_str)
        .ok_or_else(|| "kernel route result has no destination".to_string())?;
    let destination = destination
        .split_once('/')
        .map(|(destination, _)| destination)
        .unwrap_or(destination)
        .parse::<IpAddr>()
        .map(normalize_ip)
        .map_err(|_| "kernel route result has an invalid destination".to_string())?;
    if destination != normalize_ip(address) {
        return Err("kernel route result does not match the selected address".to_string());
    }
    route
        .get("dev")
        .and_then(Value::as_str)
        .ok_or_else(|| "kernel route result has no interface".to_string())
}

fn validate_interface_name_policy(device: &str) -> Result<(), String> {
    if device.is_empty()
        || device.len() > 15
        || !device
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err("kernel route selected an invalid interface name".to_string());
    }
    let lower = device.to_ascii_lowercase();
    let suspicious_prefixes = [
        "tun",
        "tap",
        "wg",
        "vpn",
        "ppp",
        "ipsec",
        "xfrm",
        "utun",
        "tailscale",
        "zerotier",
        "zt",
        "proton",
        "nordlynx",
        "mullvad",
        "warp",
        "cscotun",
        "ham",
        "veth",
        "docker",
        "podman",
        "virbr",
        "cni",
        "flannel",
        "dummy",
    ];
    if lower == "lo"
        || lower.contains("vpn")
        || suspicious_prefixes
            .iter()
            .any(|prefix| lower.starts_with(prefix))
    {
        return Err("kernel route selected a local, tunnel, or virtual interface".to_string());
    }
    Ok(())
}

pub(super) fn validate_egress_interface(device: &str) -> Result<(), String> {
    validate_interface_name_policy(device)?;
    let interface = Path::new("/sys/class/net").join(device);
    let interface_type = std::fs::read_to_string(interface.join("type"))
        .map_err(|error| format!("could not classify egress interface '{device}': {error}"))?;
    let interface_type = interface_type
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("egress interface '{device}' has an invalid type"))?;
    if !matches!(interface_type, 1 | 32) || interface.join("tun_flags").exists() {
        return Err("kernel route selected a non-Ethernet or tunnel interface".to_string());
    }
    let bridge_members = interface.join("brif");
    if interface.join("bridge").is_dir() {
        let entries = std::fs::read_dir(&bridge_members)
            .map_err(|error| format!("could not inspect bridge '{device}': {error}"))?;
        let mut count = 0_usize;
        let mut has_physical_member = false;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("could not inspect bridge member: {error}"))?;
            count += 1;
            if count > 64 {
                return Err("egress bridge member count exceeds the safety limit".to_string());
            }
            let member = entry
                .file_name()
                .into_string()
                .map_err(|_| "egress bridge member name is not UTF-8".to_string())?;
            let lower = member.to_ascii_lowercase();
            if [
                "tun",
                "tap",
                "wg",
                "vpn",
                "ipsec",
                "xfrm",
                "tailscale",
                "zerotier",
                "zt",
                "proton",
                "nordlynx",
                "mullvad",
                "warp",
                "cscotun",
            ]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
            {
                return Err("egress bridge contains a tunnel interface".to_string());
            }
            if Path::new("/sys/class/net")
                .join(&member)
                .join("device")
                .exists()
            {
                has_physical_member = true;
            }
        }
        if count == 0 || !has_physical_member {
            return Err("egress bridge has no physical network member".to_string());
        }
    }
    Ok(())
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(address)),
        address => address,
    }
}

fn ipv4_in(address: Ipv4Addr, network: [u8; 4], prefix: u8) -> bool {
    IpPrefix {
        network: IpAddr::V4(Ipv4Addr::from(network)),
        prefix,
    }
    .contains(IpAddr::V4(address))
}

fn ipv6_in(address: Ipv6Addr, network: [u16; 8], prefix: u8) -> bool {
    IpPrefix {
        network: IpAddr::V6(Ipv6Addr::new(
            network[0], network[1], network[2], network[3], network[4], network[5], network[6],
            network[7],
        )),
        prefix,
    }
    .contains(IpAddr::V6(address))
}

pub fn is_public_destination(address: IpAddr) -> bool {
    if let IpAddr::V6(address) = address
        && address.to_ipv4_mapped().is_some()
    {
        return false;
    }
    match address {
        IpAddr::V4(address) => ![
            ([0, 0, 0, 0], 8),
            ([10, 0, 0, 0], 8),
            ([100, 64, 0, 0], 10),
            ([127, 0, 0, 0], 8),
            ([169, 254, 0, 0], 16),
            ([172, 16, 0, 0], 12),
            ([192, 0, 0, 0], 24),
            ([192, 0, 2, 0], 24),
            ([192, 31, 196, 0], 24),
            ([192, 52, 193, 0], 24),
            ([192, 88, 99, 0], 24),
            ([192, 168, 0, 0], 16),
            ([192, 175, 48, 0], 24),
            ([198, 18, 0, 0], 15),
            ([198, 51, 100, 0], 24),
            ([203, 0, 113, 0], 24),
            ([224, 0, 0, 0], 4),
            ([240, 0, 0, 0], 4),
        ]
        .into_iter()
        .any(|(network, prefix)| ipv4_in(address, network, prefix)),
        IpAddr::V6(address) => {
            // Fail closed to the currently allocated global-unicast 2000::/3
            // and remove all special, transition, local, and documentation space.
            ipv6_in(address, [0x2000, 0, 0, 0, 0, 0, 0, 0], 3)
                && ![
                    ([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),
                    ([0x2001, 0x0db8, 0, 0, 0, 0, 0, 0], 32),
                    ([0x2002, 0, 0, 0, 0, 0, 0, 0], 16),
                    ([0x2620, 0x004f, 0x8000, 0, 0, 0, 0, 0], 48),
                    ([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20),
                    ([0x5f00, 0, 0, 0, 0, 0, 0, 0], 16),
                ]
                .into_iter()
                .any(|(network, prefix)| ipv6_in(address, network, prefix))
                && !ipv6_in(address, [0x0064, 0xff9b, 0, 0, 0, 0, 0, 0], 96)
                && !ipv6_in(address, [0x0064, 0xff9b, 1, 0, 0, 0, 0, 0], 48)
                && !ipv6_in(address, [0x0100, 0, 0, 0, 0, 0, 0, 0], 64)
                && !ipv6_in(address, [0xfc00, 0, 0, 0, 0, 0, 0, 0], 7)
                && !ipv6_in(address, [0xfe80, 0, 0, 0, 0, 0, 0, 0], 10)
                && !ipv6_in(address, [0xfec0, 0, 0, 0, 0, 0, 0, 0], 10)
                && !ipv6_in(address, [0xff00, 0, 0, 0, 0, 0, 0, 0], 8)
        }
    }
}

pub fn validate_public_hostname(host: &str) -> Result<(), String> {
    if host.is_empty() || host.len() > 253 || host != host.trim() {
        return Err("hostname length or whitespace is invalid".to_string());
    }
    if !host.is_ascii() || host != host.to_ascii_lowercase() || host.ends_with('.') {
        return Err("hostname must be lowercase ASCII without a trailing dot".to_string());
    }
    if host.parse::<IpAddr>().is_ok()
        || host.contains(['@', ':', '/', '\\', '?', '#', '%', '[', ']'])
    {
        return Err("numeric or malformed hostname is not allowed".to_string());
    }
    let labels = host.split('.').collect::<Vec<_>>();
    if labels.len() < 2 {
        return Err("single-label hostnames are not allowed".to_string());
    }
    for label in &labels {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("hostname contains an invalid DNS label".to_string());
        }
    }
    let reserved_suffixes = [
        "localhost",
        "local",
        "internal",
        "home",
        "lan",
        "test",
        "invalid",
        "example",
        "onion",
        "alt",
        "arpa",
    ];
    if reserved_suffixes
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
    {
        return Err("reserved or local hostname suffix is not allowed".to_string());
    }
    if labels
        .last()
        .is_some_and(|label| label.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("numeric top-level labels are not allowed".to_string());
    }
    Ok(())
}

pub(super) fn validate_dns_answers(
    answers: impl IntoIterator<Item = IpAddr>,
    routes: &RouteSnapshot,
) -> Result<Vec<IpAddr>, String> {
    let mut seen = HashSet::new();
    let mut public = Vec::new();
    let mut without_default = Vec::new();
    for answer in answers {
        if !is_public_destination(answer) {
            return Err("DNS answer set contains a non-public address".to_string());
        }
        let answer = normalize_ip(answer);
        if !seen.insert(answer) {
            continue;
        }
        match routes.classify(answer) {
            RouteDecision::Allowed => public.push(answer),
            RouteDecision::NoDefaultRoute => without_default.push(answer),
            RouteDecision::LocalOrSpecificRoute => {
                return Err("DNS answer intersects a host-local or specific route".to_string());
            }
        }
    }
    if public.is_empty() {
        if without_default.is_empty() {
            Err("DNS returned no addresses".to_string())
        } else {
            Err("DNS answers have no auditable default route".to_string())
        }
    } else {
        Ok(public)
    }
}

pub(super) fn collect_dns_answers(
    answers: impl IntoIterator<Item = SocketAddr>,
) -> Result<Vec<IpAddr>, String> {
    let mut collected = Vec::new();
    for answer in answers {
        if collected.len() == MAX_DNS_ANSWERS {
            return Err("DNS answer count exceeds the safety limit".to_string());
        }
        collected.push(answer.ip());
    }
    Ok(collected)
}
