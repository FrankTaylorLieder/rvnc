use std::net::Ipv6Addr;

/// The default VNC port (display 0).
pub const DEFAULT_PORT: u16 = 5900;

/// Parses a server address: `host`, `host:port`, `[ipv6]`, `[ipv6]:port`,
/// or a bare IPv6 address.
///
/// As in URLs, the number after a single colon is a TCP port, not a VNC
/// display number. Returns `(host, port)`, where `port` is `None` if the
/// address doesn't give one; use [`display_port`] or [`DEFAULT_PORT`].
pub fn parse_address(s: &str) -> Result<(String, Option<u16>), String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty address".into());
    }

    let (host, rest) = if let Some(stripped) = s.strip_prefix('[') {
        let end = stripped
            .find(']')
            .ok_or_else(|| format!("unterminated '[' in {s:?}"))?;
        (&stripped[..end], &stripped[end + 1..])
    } else if s.matches(':').count() > 1 {
        // More than one colon without brackets can only be a bare IPv6
        // address. Anything else (e.g. the `host::port` form some viewers
        // use) is rejected.
        if s.parse::<Ipv6Addr>().is_err() {
            return Err(format!(
                "invalid address {s:?}: use host:port, and [brackets] around IPv6 addresses"
            ));
        }
        (s, "")
    } else if let Some(i) = s.find(':') {
        (&s[..i], &s[i..])
    } else {
        (s, "")
    };

    if host.is_empty() {
        return Err(format!("missing host in {s:?}"));
    }

    let port = if rest.is_empty() {
        None
    } else if let Some(p) = rest.strip_prefix(':') {
        Some(
            p.parse::<u16>()
                .map_err(|_| format!("invalid port {p:?} in {s:?}"))?,
        )
    } else {
        return Err(format!("unexpected {rest:?} after host in {s:?}"));
    };

    Ok((host.to_owned(), port))
}

/// The port for VNC display `n` (5900 + n), if it fits in a `u16`.
pub fn display_port(n: u16) -> Option<u16> {
    DEFAULT_PORT.checked_add(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> (String, Option<u16>) {
        parse_address(s).unwrap()
    }

    #[test]
    fn forms() {
        assert_eq!(ok("myhost"), ("myhost".into(), None));
        assert_eq!(ok("myhost:1"), ("myhost".into(), Some(1)));
        assert_eq!(ok("myhost:5901"), ("myhost".into(), Some(5901)));
        assert_eq!(ok("10.0.0.1:5902"), ("10.0.0.1".into(), Some(5902)));
        assert_eq!(ok("[::1]"), ("::1".into(), None));
        assert_eq!(ok("[::1]:5903"), ("::1".into(), Some(5903)));
        assert_eq!(ok("::1"), ("::1".into(), None));
        assert_eq!(ok("fe80::1"), ("fe80::1".into(), None));
        assert_eq!(ok("fe80::1:2:3"), ("fe80::1:2:3".into(), None));
    }

    #[test]
    fn errors() {
        assert!(parse_address("").is_err());
        assert!(parse_address(":5900").is_err());
        assert!(parse_address("host:x").is_err());
        assert!(parse_address("host:65536").is_err());
        assert!(parse_address("[::1").is_err());
        assert!(parse_address("[::1]x").is_err());
        assert!(parse_address("[::1]::6000").is_err());
    }

    #[test]
    fn double_colon_port_form_is_rejected_helpfully() {
        let e = parse_address("localhost::16602").unwrap_err();
        assert!(e.contains("host:port"), "{e}");
    }

    #[test]
    fn displays() {
        assert_eq!(display_port(0), Some(5900));
        assert_eq!(display_port(1), Some(5901));
        assert_eq!(display_port(u16::MAX - 5900), Some(u16::MAX));
        assert_eq!(display_port(u16::MAX - 5899), None);
    }
}
