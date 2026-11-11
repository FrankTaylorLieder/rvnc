/// The default VNC port (display 0).
pub const DEFAULT_PORT: u16 = 5900;

/// Parses a VNC server address in the conventional forms:
///
/// - `host` → port 5900
/// - `host:N` → port 5900 + N when N < 100 (display number), else port N
/// - `host::port` → that port
/// - `[ipv6]`, `[ipv6]:N`, `[ipv6]::port`, or a bare IPv6 address
///
/// Returns `(host, port)` or a description of what is wrong.
pub fn parse_address(s: &str) -> Result<(String, u16), String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty address".into());
    }

    let (host, rest) = if let Some(stripped) = s.strip_prefix('[') {
        let end = stripped
            .find(']')
            .ok_or_else(|| format!("unterminated '[' in {s:?}"))?;
        (&stripped[..end], &stripped[end + 1..])
    } else if s.matches(':').count() > 2 || s.starts_with("::") {
        // Bare IPv6 address with no port.
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
        DEFAULT_PORT
    } else if let Some(p) = rest.strip_prefix("::") {
        p.parse::<u16>()
            .map_err(|_| format!("invalid port {p:?} in {s:?}"))?
    } else if let Some(d) = rest.strip_prefix(':') {
        let n = d
            .parse::<u16>()
            .map_err(|_| format!("invalid display or port {d:?} in {s:?}"))?;
        if n < 100 { DEFAULT_PORT + n } else { n }
    } else {
        return Err(format!("unexpected {rest:?} after host in {s:?}"));
    };

    Ok((host.to_owned(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> (String, u16) {
        parse_address(s).unwrap()
    }

    #[test]
    fn forms() {
        assert_eq!(ok("myhost"), ("myhost".into(), 5900));
        assert_eq!(ok("myhost:1"), ("myhost".into(), 5901));
        assert_eq!(ok("myhost:99"), ("myhost".into(), 5999));
        assert_eq!(ok("myhost:5905"), ("myhost".into(), 5905));
        assert_eq!(ok("myhost::5"), ("myhost".into(), 5));
        assert_eq!(ok("10.0.0.1:2"), ("10.0.0.1".into(), 5902));
        assert_eq!(ok("[::1]"), ("::1".into(), 5900));
        assert_eq!(ok("[::1]:3"), ("::1".into(), 5903));
        assert_eq!(ok("[fe80::1]::6000"), ("fe80::1".into(), 6000));
        assert_eq!(ok("::1"), ("::1".into(), 5900));
        assert_eq!(ok("fe80::1:2:3"), ("fe80::1:2:3".into(), 5900));
    }

    #[test]
    fn errors() {
        assert!(parse_address("").is_err());
        assert!(parse_address(":1").is_err());
        assert!(parse_address("host:x").is_err());
        assert!(parse_address("host::99999").is_err());
        assert!(parse_address("[::1").is_err());
        assert!(parse_address("[::1]x").is_err());
    }
}
