//! Server settings, read from `PIECEOFF_*` environment variables.
//!
//! A mistyped value stops the server with a message that names the variable,
//! rather than being ignored: a quota or limit that silently falls back to its
//! default is the kind of mistake nobody notices until it matters.

use std::{net::SocketAddr, path::PathBuf};

pub const DEFAULT_BIND: &str = "0.0.0.0:28878";
pub const DEFAULT_MEDIA_QUOTA_BYTES: u64 = 50 * 1024 * 1024 * 1024;
/// The largest single media file a room may hold. Streaming keeps memory flat
/// whatever this is; it bounds disk use and how long one transfer can run.
pub const DEFAULT_MAX_MEDIA_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const DEFAULT_ROOM_TTL_DAYS: u64 = 90;
pub const DEFAULT_MAX_ROOMS: usize = 500;
pub const DEFAULT_MAX_CONNECTIONS: usize = 400;
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_CONNECTIONS_PER_MINUTE: u32 = 60;
pub const DEFAULT_FAILED_JOINS_PER_HOUR: u32 = 20;

/// The smallest message cap that still fits a 400-piece puzzle snapshot.
pub const MIN_MAX_MESSAGE_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub media_quota_bytes: u64,
    /// Largest single uploaded file.
    pub max_media_bytes: u64,
    /// Days an unused room is kept; 0 keeps rooms forever.
    pub room_ttl_days: u64,
    pub max_rooms: usize,
    pub max_connections: usize,
    pub max_message_bytes: usize,
    /// Believe `X-Forwarded-For` (set only behind a proxy you control).
    pub trust_proxy: bool,
    /// Browser origins allowed to open a room socket. Native clients send no
    /// origin and are always allowed.
    pub allowed_origins: Vec<String>,
    pub connections_per_minute: u32,
    pub failed_joins_per_hour: u32,
}

impl Config {
    pub fn defaults(data_dir: PathBuf) -> Self {
        Self {
            bind: DEFAULT_BIND.parse().expect("default bind address is valid"),
            data_dir,
            media_quota_bytes: DEFAULT_MEDIA_QUOTA_BYTES,
            max_media_bytes: DEFAULT_MAX_MEDIA_BYTES,
            room_ttl_days: DEFAULT_ROOM_TTL_DAYS,
            max_rooms: DEFAULT_MAX_ROOMS,
            max_connections: DEFAULT_MAX_CONNECTIONS,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            trust_proxy: false,
            allowed_origins: Vec::new(),
            connections_per_minute: DEFAULT_CONNECTIONS_PER_MINUTE,
            failed_joins_per_hour: DEFAULT_FAILED_JOINS_PER_HOUR,
        }
    }

    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Build a configuration from any variable source (the environment, or a
    /// map in tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let get = |name: &str| get(name).map(|value| value.trim().to_owned());
        let mut config = Self::defaults(
            get("PIECEOFF_DATA_DIR")
                .filter(|value| !value.is_empty())
                .map_or_else(|| PathBuf::from("pieceoff-data"), PathBuf::from),
        );
        if let Some(value) = get("PIECEOFF_BIND").filter(|value| !value.is_empty()) {
            config.bind = value.parse().map_err(|_| {
                format!("PIECEOFF_BIND must look like 0.0.0.0:28878 or [::]:28878, not '{value}'")
            })?;
        }
        fn number<T: std::str::FromStr>(
            get: &dyn Fn(&str) -> Option<String>,
            name: &str,
            target: &mut T,
        ) -> Result<(), String> {
            if let Some(value) = get(name).filter(|value| !value.is_empty()) {
                *target = value
                    .parse()
                    .map_err(|_| format!("{name} must be a whole number, not '{value}'"))?;
            }
            Ok(())
        }
        number(
            &get,
            "PIECEOFF_MEDIA_QUOTA_BYTES",
            &mut config.media_quota_bytes,
        )?;
        number(
            &get,
            "PIECEOFF_MAX_MEDIA_BYTES",
            &mut config.max_media_bytes,
        )?;
        number(&get, "PIECEOFF_ROOM_TTL_DAYS", &mut config.room_ttl_days)?;
        number(&get, "PIECEOFF_MAX_ROOMS", &mut config.max_rooms)?;
        number(
            &get,
            "PIECEOFF_MAX_CONNECTIONS",
            &mut config.max_connections,
        )?;
        number(
            &get,
            "PIECEOFF_MAX_MESSAGE_BYTES",
            &mut config.max_message_bytes,
        )?;
        number(
            &get,
            "PIECEOFF_CONNECTIONS_PER_MINUTE",
            &mut config.connections_per_minute,
        )?;
        number(
            &get,
            "PIECEOFF_FAILED_JOINS_PER_HOUR",
            &mut config.failed_joins_per_hour,
        )?;
        if let Some(value) = get("PIECEOFF_TRUST_PROXY").filter(|value| !value.is_empty()) {
            config.trust_proxy = match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => {
                    return Err(format!(
                        "PIECEOFF_TRUST_PROXY must be 1 or 0 (or true/false), not '{value}'"
                    ));
                }
            };
        }
        if let Some(value) = get("PIECEOFF_ALLOWED_ORIGINS") {
            config.allowed_origins = value
                .split(',')
                .map(|origin| origin.trim().trim_end_matches('/').to_ascii_lowercase())
                .filter(|origin| !origin.is_empty())
                .collect();
            if let Some(bad) = config
                .allowed_origins
                .iter()
                .find(|origin| !origin.starts_with("http://") && !origin.starts_with("https://"))
            {
                return Err(format!(
                    "PIECEOFF_ALLOWED_ORIGINS entries must start with http:// or https://, not '{bad}'"
                ));
            }
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if self.max_rooms == 0 {
            return Err("PIECEOFF_MAX_ROOMS must be at least 1".into());
        }
        if self.max_media_bytes == 0 {
            return Err("PIECEOFF_MAX_MEDIA_BYTES must be at least 1".into());
        }
        if self.max_connections == 0 {
            return Err("PIECEOFF_MAX_CONNECTIONS must be at least 1".into());
        }
        if self.max_message_bytes < MIN_MAX_MESSAGE_BYTES {
            return Err(format!(
                "PIECEOFF_MAX_MESSAGE_BYTES must be at least {MIN_MAX_MESSAGE_BYTES}, or large puzzles could not be created"
            ));
        }
        if self.connections_per_minute == 0 || self.failed_joins_per_hour == 0 {
            return Err(
                "PIECEOFF_CONNECTIONS_PER_MINUTE and PIECEOFF_FAILED_JOINS_PER_HOUR must be at least 1"
                    .into(),
            );
        }
        Ok(())
    }

    /// Lines for the startup log, so what the server is running with is
    /// visible without reading its environment.
    pub fn summary(&self) -> Vec<String> {
        let ttl = if self.room_ttl_days == 0 {
            "never".to_owned()
        } else {
            format!("{} days", self.room_ttl_days)
        };
        let origins = if self.allowed_origins.is_empty() {
            "native clients only".to_owned()
        } else {
            self.allowed_origins.join(", ")
        };
        vec![
            format!("listening on {}", self.bind),
            format!("data directory {}", self.data_dir.display()),
            format!(
                "media quota {} GiB, single file up to {} GiB, rooms expire after {ttl}",
                self.media_quota_bytes / (1024 * 1024 * 1024),
                self.max_media_bytes / (1024 * 1024 * 1024)
            ),
            format!(
                "at most {} rooms and {} connections; messages up to {} KiB",
                self.max_rooms,
                self.max_connections,
                self.max_message_bytes / 1024
            ),
            format!(
                "per address: {} connections a minute, {} failed joins an hour",
                self.connections_per_minute, self.failed_joins_per_hour
            ),
            format!(
                "client addresses from {}; browser origins: {origins}",
                if self.trust_proxy {
                    "X-Forwarded-For (trusted proxy)"
                } else {
                    "the socket"
                }
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn from(pairs: &[(&str, &str)]) -> Result<Config, String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Config::from_lookup(|name| map.get(name).cloned())
    }

    #[test]
    fn an_empty_environment_gives_the_documented_defaults() {
        let config = from(&[]).unwrap();
        assert_eq!(config, Config::defaults(PathBuf::from("pieceoff-data")));
        assert_eq!(config.bind.port(), 28878);
        assert_eq!(config.media_quota_bytes, 50 * 1024 * 1024 * 1024);
        assert_eq!(config.max_media_bytes, 16 * 1024 * 1024 * 1024);
        assert_eq!(config.room_ttl_days, 90);
        assert!(!config.trust_proxy);
    }

    #[test]
    fn every_variable_is_read() {
        let config = from(&[
            ("PIECEOFF_BIND", "127.0.0.1:9000"),
            ("PIECEOFF_DATA_DIR", "/var/lib/pieceoff"),
            ("PIECEOFF_MEDIA_QUOTA_BYTES", "1000"),
            ("PIECEOFF_MAX_MEDIA_BYTES", "900"),
            ("PIECEOFF_ROOM_TTL_DAYS", "0"),
            ("PIECEOFF_MAX_ROOMS", "5"),
            ("PIECEOFF_MAX_CONNECTIONS", "6"),
            ("PIECEOFF_MAX_MESSAGE_BYTES", "300000"),
            ("PIECEOFF_TRUST_PROXY", "yes"),
            (
                "PIECEOFF_ALLOWED_ORIGINS",
                "https://Play.Example.com/, http://localhost:8080",
            ),
            ("PIECEOFF_CONNECTIONS_PER_MINUTE", "7"),
            ("PIECEOFF_FAILED_JOINS_PER_HOUR", "8"),
        ])
        .unwrap();
        assert_eq!(config.bind, "127.0.0.1:9000".parse().unwrap());
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/pieceoff"));
        assert_eq!(config.media_quota_bytes, 1000);
        assert_eq!(config.max_media_bytes, 900);
        assert_eq!(config.room_ttl_days, 0);
        assert_eq!((config.max_rooms, config.max_connections), (5, 6));
        assert_eq!(config.max_message_bytes, 300_000);
        assert!(config.trust_proxy);
        assert_eq!(
            config.allowed_origins,
            ["https://play.example.com", "http://localhost:8080"]
        );
        assert_eq!(
            (config.connections_per_minute, config.failed_joins_per_hour),
            (7, 8)
        );
    }

    #[test]
    fn a_bad_value_names_its_variable_instead_of_falling_back() {
        for (name, value, needle) in [
            ("PIECEOFF_BIND", "localhost", "PIECEOFF_BIND"),
            (
                "PIECEOFF_MEDIA_QUOTA_BYTES",
                "50GB",
                "PIECEOFF_MEDIA_QUOTA_BYTES",
            ),
            ("PIECEOFF_ROOM_TTL_DAYS", "-1", "PIECEOFF_ROOM_TTL_DAYS"),
            ("PIECEOFF_MAX_ROOMS", "0", "at least 1"),
            ("PIECEOFF_MAX_MEDIA_BYTES", "0", "at least 1"),
            (
                "PIECEOFF_MAX_MEDIA_BYTES",
                "10GB",
                "PIECEOFF_MAX_MEDIA_BYTES",
            ),
            (
                "PIECEOFF_MAX_CONNECTIONS",
                "many",
                "PIECEOFF_MAX_CONNECTIONS",
            ),
            ("PIECEOFF_MAX_MESSAGE_BYTES", "1000", "large puzzles"),
            ("PIECEOFF_TRUST_PROXY", "maybe", "PIECEOFF_TRUST_PROXY"),
            ("PIECEOFF_ALLOWED_ORIGINS", "example.com", "http://"),
            ("PIECEOFF_CONNECTIONS_PER_MINUTE", "0", "at least 1"),
        ] {
            let error = from(&[(name, value)]).unwrap_err();
            assert!(error.contains(needle), "{name}={value}: {error}");
        }
    }

    #[test]
    fn blank_values_mean_unset_and_the_summary_shows_what_is_in_force() {
        let config = from(&[("PIECEOFF_MAX_ROOMS", "  "), ("PIECEOFF_BIND", "")]).unwrap();
        assert_eq!(config.max_rooms, DEFAULT_MAX_ROOMS);
        let summary = config.summary().join("\n");
        assert!(summary.contains("listening on 0.0.0.0:28878"));
        assert!(summary.contains("50 GiB"));
        assert!(summary.contains("native clients only"));
    }
}
