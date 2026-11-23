//! Configuration for the telemetry MCP server.
//!
//! The server does NOT read `.env` itself: when it runs under the agent, the
//! agent passes the values as environment variables of the child process.
//! Parsing is written against a lookup function so tests need no real environment.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    /// Folder holding logs.json, deployments.json, runbooks/, release-checklist.md.
    pub data_dir: PathBuf,
    /// SQLite file for incident tickets.
    pub tickets_db: PathBuf,
    /// "Now" for time-window queries. Validated (parsed as a timestamp) in Phase 3.
    pub reference_time: String,
    pub log_level: String,
}

impl ServerConfig {
    pub fn from_env(base_dir: &Path) -> Self {
        Self::from_lookup(base_dir, |name| std::env::var(name).ok())
    }

    pub fn from_lookup(base_dir: &Path, lookup: impl Fn(&str) -> Option<String>) -> Self {
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

        Self {
            data_dir: path("DATA_DIR", "mock_data"),
            tickets_db: path("TICKETS_DB", r"data\tickets.db"),
            reference_time: get("OPS_REFERENCE_TIME", "2026-11-20T12:00:00Z"),
            log_level: get("LOG_LEVEL", "info"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn load(pairs: &[(&str, &str)]) -> ServerConfig {
        let map: HashMap<&str, &str> = pairs.iter().copied().collect();
        ServerConfig::from_lookup(Path::new(r"C:\project"), |k| {
            map.get(k).map(|v| v.to_string())
        })
    }

    #[test]
    fn defaults_resolve_against_the_base_dir() {
        let cfg = load(&[]);
        assert_eq!(cfg.data_dir, PathBuf::from(r"C:\project\mock_data"));
        assert_eq!(cfg.tickets_db, PathBuf::from(r"C:\project\data\tickets.db"));
        assert_eq!(cfg.reference_time, "2026-11-20T12:00:00Z");
        assert_eq!(cfg.log_level, "info");
    }

    #[test]
    fn overrides_and_absolute_paths_are_respected() {
        let cfg = load(&[
            ("DATA_DIR", r"D:\seed"),
            ("TICKETS_DB", r"tmp\t.db"),
            ("OPS_REFERENCE_TIME", "now"),
            ("LOG_LEVEL", "  "), // blank -> default
        ]);
        assert_eq!(cfg.data_dir, PathBuf::from(r"D:\seed"));
        assert_eq!(cfg.tickets_db, PathBuf::from(r"C:\project\tmp\t.db"));
        assert_eq!(cfg.reference_time, "now");
        assert_eq!(cfg.log_level, "info");
    }
}
