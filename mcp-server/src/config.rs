//! Configuration for the telemetry MCP server.
//!
//! The server does NOT read `.env` itself: when it runs under the agent, the
//! agent passes the values as environment variables of the child process.
//! Parsing is written against a lookup function so tests need no real environment.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use thiserror::Error;

/// Default "now" for time-window queries; the seed data is built around it.
pub const DEFAULT_REFERENCE_TIME: &str = "2026-11-20T12:00:00Z";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error(
        "OPS_REFERENCE_TIME=`{value}` is not valid: use an RFC 3339 UTC timestamp \
         such as {DEFAULT_REFERENCE_TIME}, or the word `now`"
    )]
    InvalidReferenceTime { value: String },
}

/// The instant that time windows ("last 24 hours") are measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceTime {
    /// A fixed instant, so answers are reproducible (the default).
    Fixed(DateTime<Utc>),
    /// The real clock.
    Now,
}

impl ReferenceTime {
    pub fn parse(value: &str) -> Result<Self, ConfigError> {
        let value = value.trim();
        if value.eq_ignore_ascii_case("now") {
            return Ok(Self::Now);
        }
        DateTime::parse_from_rfc3339(value)
            .map(|t| Self::Fixed(t.with_timezone(&Utc)))
            .map_err(|_| ConfigError::InvalidReferenceTime {
                value: value.to_string(),
            })
    }

    pub fn now(&self) -> DateTime<Utc> {
        match self {
            Self::Fixed(t) => *t,
            Self::Now => Utc::now(),
        }
    }
}

impl std::fmt::Display for ReferenceTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fixed(t) => write!(
                f,
                "{}",
                t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            ),
            Self::Now => f.write_str("now (real clock)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Folder holding logs.json, deployments.json, runbooks/, release-checklist.md.
    pub data_dir: PathBuf,
    /// SQLite file for incident tickets.
    pub tickets_db: PathBuf,
    pub reference_time: ReferenceTime,
    pub log_level: String,
}

impl ServerConfig {
    pub fn from_env(base_dir: &Path) -> Result<Self, ConfigError> {
        Self::from_lookup(base_dir, |name| std::env::var(name).ok())
    }

    pub fn from_lookup(
        base_dir: &Path,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let get = |name: &str, default: &str| {
            lookup(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        let path = |name: &str, default: &str| {
            let p = PathBuf::from(get(name, default));
            if p.is_absolute() { p } else { base_dir.join(p) }
        };

        Ok(Self {
            data_dir: path("DATA_DIR", "mock_data"),
            tickets_db: path("TICKETS_DB", r"data\tickets.db"),
            reference_time: ReferenceTime::parse(&get(
                "OPS_REFERENCE_TIME",
                DEFAULT_REFERENCE_TIME,
            ))?,
            log_level: get("LOG_LEVEL", "info"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(pairs: &[(&str, &str)]) -> Result<ServerConfig, ConfigError> {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        ServerConfig::from_lookup(Path::new(r"C:\project"), |k| {
            map.get(k).map(|v| v.to_string())
        })
    }

    #[test]
    fn defaults_resolve_against_the_base_dir() {
        let cfg = load(&[]).unwrap();
        assert_eq!(cfg.data_dir, PathBuf::from(r"C:\project\mock_data"));
        assert_eq!(cfg.tickets_db, PathBuf::from(r"C:\project\data\tickets.db"));
        assert_eq!(
            cfg.reference_time.to_string(),
            "2026-11-20T12:00:00Z",
            "default anchor"
        );
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn overrides_and_absolute_paths_are_respected() {
        let cfg = load(&[
            ("DATA_DIR", r"D:\seed"),
            ("TICKETS_DB", r"tmp\t.db"),
            ("OPS_REFERENCE_TIME", "NOW"),
            ("LOG_LEVEL", "  "), // blank -> default
        ])
        .unwrap();
        assert_eq!(cfg.data_dir, PathBuf::from(r"D:\seed"));
        assert_eq!(cfg.tickets_db, PathBuf::from(r"C:\project\tmp\t.db"));
        assert_eq!(cfg.reference_time, ReferenceTime::Now);
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn offsets_are_normalised_to_utc() {
        let cfg = load(&[("OPS_REFERENCE_TIME", "2026-11-20T14:00:00+02:00")]).unwrap();
        assert_eq!(cfg.reference_time.to_string(), "2026-11-20T12:00:00Z");
    }

    #[test]
    fn an_invalid_reference_time_is_rejected() {
        let err = load(&[("OPS_REFERENCE_TIME", "yesterday")]).unwrap_err();
        assert!(err.to_string().contains("yesterday"), "{err}");
    }
}
