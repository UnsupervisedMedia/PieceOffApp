//! The room server address the client connects to: what the player may type,
//! and which source (saved setting, environment, built-in default) wins.

pub(crate) const DEFAULT_PORT: u16 = 28878;
pub(crate) const DEFAULT_SERVER_URL: &str = "ws://127.0.0.1:28878/ws";

/// Turn what a player typed into a `ws://` or `wss://` URL.
///
/// A bare `host`, `host:port` or IPv6 address gets the default port and the
/// `/ws` path. An explicit scheme is kept as typed (so `wss://host` stays on
/// 443); `http(s)://` is accepted as `ws(s)://`.
pub(crate) fn normalize_server_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("Enter a server address".into());
    }
    if input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("Server address cannot contain spaces".into());
    }
    let (scheme, rest, explicit) = match input.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = match scheme.to_ascii_lowercase().as_str() {
                "ws" | "http" => "ws",
                "wss" | "https" => "wss",
                _ => return Err(format!("Use ws:// or wss://, not {scheme}://")),
            };
            (scheme, rest, true)
        }
        None => ("ws", input, false),
    };
    let (authority, path) = match rest.find('/') {
        Some(index) => rest.split_at(index),
        None => (rest, ""),
    };
    let authority = normalize_authority(authority, !explicit)?;
    let path = if path.is_empty() || path == "/" {
        "/ws"
    } else {
        path
    };
    Ok(format!("{scheme}://{authority}{path}"))
}

fn normalize_authority(authority: &str, default_port: bool) -> Result<String, String> {
    if authority.is_empty() {
        return Err("Server address has no host".into());
    }
    if authority.contains('@') {
        return Err("Server address cannot contain a user name".into());
    }
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (address, tail) = inner
            .split_once(']')
            .ok_or("IPv6 address is missing its closing ]")?;
        let port = match tail {
            "" => None,
            _ => Some(tail.strip_prefix(':').ok_or("Unexpected text after ]")?),
        };
        (bracketed(address)?, port)
    } else {
        match authority.matches(':').count() {
            0 => (hostname(authority)?, None),
            1 => {
                let (host, port) = authority.split_once(':').unwrap_or_default();
                (hostname(host)?, Some(port))
            }
            _ => (bracketed(authority)?, None),
        }
    };
    let port = match port {
        Some(text) => Some(match text.parse::<u16>() {
            Ok(port) if port != 0 => port,
            _ => return Err(format!("'{text}' is not a valid port")),
        }),
        None => default_port.then_some(DEFAULT_PORT),
    };
    Ok(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn hostname(host: &str) -> Result<String, String> {
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_'));
    if valid {
        Ok(host.to_owned())
    } else {
        Err(format!("'{host}' is not a valid host name or IP address"))
    }
}

fn bracketed(address: &str) -> Result<String, String> {
    let valid = !address.is_empty()
        && address.contains(':')
        && address
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.' | '%'));
    if valid {
        Ok(format!("[{address}]"))
    } else {
        Err(format!("'{address}' is not a valid IPv6 address"))
    }
}

/// The server address to start with and where it came from. A saved choice
/// wins over the environment, which wins over the built-in default.
pub(crate) fn initial_server_url(saved: Option<&str>, env: Option<&str>) -> (String, &'static str) {
    let usable = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    if let Some(url) = usable(saved) {
        (url, "saved setting")
    } else if let Some(url) = usable(env) {
        (url, "PIECEOFF_SERVER_URL")
    } else {
        (DEFAULT_SERVER_URL.to_owned(), "built-in default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(input: &str) -> String {
        normalize_server_url(input).unwrap()
    }

    #[test]
    fn bare_hosts_get_default_port_and_path() {
        assert_eq!(n("192.168.1.5"), "ws://192.168.1.5:28878/ws");
        assert_eq!(n("  game.example  "), "ws://game.example:28878/ws");
        assert_eq!(n("game.example:9000"), "ws://game.example:9000/ws");
        assert_eq!(n("game.example/"), "ws://game.example:28878/ws");
        assert_eq!(
            n("game.example:9000/custom"),
            "ws://game.example:9000/custom"
        );
    }

    #[test]
    fn explicit_schemes_are_kept_and_path_defaults() {
        assert_eq!(n("ws://10.0.0.2:8787/ws"), "ws://10.0.0.2:8787/ws");
        assert_eq!(n("wss://pieceoff.example"), "wss://pieceoff.example/ws");
        assert_eq!(n("WSS://pieceoff.example/ws"), "wss://pieceoff.example/ws");
        assert_eq!(n("http://box.lan:8787"), "ws://box.lan:8787/ws");
        assert_eq!(n("https://pieceoff.example"), "wss://pieceoff.example/ws");
    }

    #[test]
    fn ipv6_addresses() {
        assert_eq!(n("::1"), "ws://[::1]:28878/ws");
        assert_eq!(n("[::1]"), "ws://[::1]:28878/ws");
        assert_eq!(n("[fe80::1]:9000"), "ws://[fe80::1]:9000/ws");
        assert_eq!(n("ws://[::1]:8787/ws"), "ws://[::1]:8787/ws");
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        for bad in [
            "",
            "   ",
            "ftp://host",
            "ws://",
            "host name",
            "user@host",
            "host:0",
            "host:99999",
            "host:abc",
            "[::1",
            "[::1]x",
            "ho$t",
        ] {
            assert!(normalize_server_url(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn normalised_urls_feed_the_media_url_builder() {
        let ws = n("192.168.1.5");
        assert_eq!(
            crate::package::http_media_url(&ws, "/media/x").unwrap(),
            "http://192.168.1.5:28878/media/x"
        );
    }

    #[test]
    fn saved_choice_beats_environment_beats_default() {
        let (url, source) = initial_server_url(Some("ws://saved:1/ws"), Some("ws://env:2/ws"));
        assert_eq!((url.as_str(), source), ("ws://saved:1/ws", "saved setting"));
        let (url, source) = initial_server_url(None, Some("ws://env:2/ws"));
        assert_eq!(
            (url.as_str(), source),
            ("ws://env:2/ws", "PIECEOFF_SERVER_URL")
        );
        let (url, source) = initial_server_url(None, None);
        assert_eq!(
            (url.as_str(), source),
            (DEFAULT_SERVER_URL, "built-in default")
        );
        // An empty saved value is not a choice.
        let (_, source) = initial_server_url(Some("  "), None);
        assert_eq!(source, "built-in default");
    }
}
