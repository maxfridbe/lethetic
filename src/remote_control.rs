//! Remote-control bind discovery shared by the `--rc` terminal chooser and
//! the command-palette setup dialog.

use std::net::IpAddr;

pub const DEFAULT_RC_PORT: u16 = 11223;

/// One selectable bind identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindChoice {
    pub host: String,
    pub note: String,
}

/// Normalise a user-supplied `--rc` target into the exact HTTPS URL form the
/// WFE security layer expects. Accepts `host`, `host:port`, `[v6]`,
/// `[v6]:port`, and full `https://` URLs.
pub fn normalize_rc_target(value: &str) -> String {
    let value = value.trim();
    let (scheme, authority) = match value.split_once("://") {
        Some((scheme, rest)) => (Some(scheme), rest),
        None => (None, value),
    };
    let has_port = if authority.starts_with('[') {
        authority
            .rsplit_once("]:")
            .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
    } else {
        authority
            .rsplit_once(':')
            .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
    };
    let authority = if has_port {
        authority.to_string()
    } else {
        format!("{authority}:{DEFAULT_RC_PORT}")
    };
    // Any explicit scheme is passed through so the security layer can reject
    // it with its own precise message; a bare authority gets https.
    match scheme {
        Some(scheme) => format!("{scheme}://{authority}"),
        None => format!("https://{authority}"),
    }
}

fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        let result =
            unsafe { libc::gethostname(buffer.as_mut_ptr().cast::<libc::c_char>(), buffer.len()) };
        if result != 0 {
            return None;
        }
        let end = buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len());
        let name = String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
        let valid = !name.is_empty()
            && name != "localhost"
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.'));
        valid.then_some(name)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn interface_addresses() -> Vec<(String, IpAddr)> {
    use std::net::{Ipv4Addr, Ipv6Addr};

    let mut out = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` with a linked list that is released by
    // freeifaddrs below; every pointer read is checked for null first.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return out;
    }
    let mut cursor = list;
    while !cursor.is_null() {
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        if entry.ifa_addr.is_null() || entry.ifa_flags & (libc::IFF_UP as u32) == 0 {
            continue;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
        if family == libc::AF_INET {
            let address = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in) };
            let ip = Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr));
            out.push((name, IpAddr::V4(ip)));
        } else if family == libc::AF_INET6 {
            let address = unsafe { &*(entry.ifa_addr as *const libc::sockaddr_in6) };
            let ip = Ipv6Addr::from(address.sin6_addr.s6_addr);
            if !ip.is_unicast_link_local() {
                out.push((name, IpAddr::V6(ip)));
            }
        }
    }
    unsafe { libc::freeifaddrs(list) };
    out
}

#[cfg(not(unix))]
fn interface_addresses() -> Vec<(String, IpAddr)> {
    Vec::new()
}

/// Loopback first, then IPv4 interfaces, then IPv6, then the host name.
pub fn bind_choices() -> Vec<BindChoice> {
    let mut choices = vec![BindChoice {
        host: "127.0.0.1".to_string(),
        note: "this machine only".to_string(),
    }];
    let mut addresses = interface_addresses();
    addresses.retain(|(_, ip)| !ip.is_loopback() && !ip.is_unspecified());
    addresses.sort_by_key(|(name, ip)| (ip.is_ipv6(), name.clone()));
    for (name, ip) in addresses {
        let host = match ip {
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) => format!("[{v6}]"),
        };
        if choices.iter().any(|choice| choice.host == host) {
            continue;
        }
        let reach = match ip {
            IpAddr::V4(v4) if v4.is_private() => "LAN / VPN",
            IpAddr::V4(_) => "public",
            IpAddr::V6(_) => "IPv6",
        };
        choices.push(BindChoice {
            host,
            note: format!("{name}, {reach}"),
        });
    }
    if let Some(name) = hostname() {
        choices.push(BindChoice {
            host: name,
            note: "host name, resolved by DNS at startup".to_string(),
        });
    }
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_normalize_to_exact_https_urls() {
        assert_eq!(normalize_rc_target("brainiac"), "https://brainiac:11223");
        assert_eq!(
            normalize_rc_target("brainiac:9443"),
            "https://brainiac:9443"
        );
        assert_eq!(normalize_rc_target("10.0.0.5"), "https://10.0.0.5:11223");
        assert_eq!(normalize_rc_target("[::1]"), "https://[::1]:11223");
        assert_eq!(normalize_rc_target("[::1]:8443"), "https://[::1]:8443");
        assert_eq!(
            normalize_rc_target("https://127.0.0.1:11223"),
            "https://127.0.0.1:11223"
        );
        assert_eq!(
            normalize_rc_target("https://brainiac"),
            "https://brainiac:11223"
        );
        assert_eq!(normalize_rc_target("http://x:1"), "http://x:1");
    }

    #[test]
    fn choices_default_to_loopback_first() {
        let choices = bind_choices();
        assert_eq!(choices[0].host, "127.0.0.1");
        assert!(choices.iter().all(|c| !c.host.is_empty()));
    }
}
