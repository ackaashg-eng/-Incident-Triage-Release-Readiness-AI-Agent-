//! Configuration for the agent orchestrator.
//!
//! Values come from environment variables. A local `.env` file in the working
//! directory is loaded first; real environment variables always win over `.env`.
//!
//! Parsing is written against a *lookup function* (`Fn(&str) -> Option<String>`)
//! instead of reading `std::env` directly, so tests can feed in a plain map
//! without touching the process environment.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use thiserror::Error;

/// Placeholder written in `.env.example`; treated as "no key".
pub const API_KEY_PLACEHOLDER: &str = "paste-your-anthropic-api-key-here";

const DEFAULT_MODEL: &str = "claude-opus-5-5";
const DEFAULT_MAX_ITERATIONS: u32 = 8;
const DEFAULT_TOOL_TIMEOUT_SECS: u64 = 10;
const DEFAULT_LOG_LEVEL: &str = "warn";

/// Everything that can be wrong with the configuration.
/// Each message tells the user how to fix it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error(
        "ANTHROPIC_API_KEY is not set. Run `scripts\\setup.ps1` to paste your key, or add it to the .env file."
    )]
    MissingApiKey,

    #[error(
        "ANTHROPIC_API_KEY still contains the placeholder from .env.example. Run `scripts\\setup.ps1` to paste your real key."
    )]
    PlaceholderApiKey,

    #[error("{name} has an invalid value `{value}`: {reason}")]
    InvalidValue {
        name: &'static str,
        value: String,
        reason: String,
    },

    #[error("could not read the .env file: {0}")]
    EnvFile(String),
}

/// The Anthropic API key.
///
/// The inner string is private and `Debug`/`Display` print `***`, so the key
/// cannot leak through logging, error messages or `{:?}` formatting by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// The real key, for the HTTP header only. Never log the result.
    #[allow(dead_code)] // first used by the Claude API client in Phase 5
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(***)")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// How much thinking/effort Claude spends (`output_config.effort`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl FromStr for Effort {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            _ => Err("expected one of: low, medium, high, xhigh, max".to_string()),
        }
    }
}

impl fmt::Display for Effort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        })
    }
}

/// The validated configuration of the orchestrator.
#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: ApiKey,
    pub model: String,
    pub effort: Effort,
    /// Server-side refusal fallbacks (`fallbacks: "default"`).
    pub fallbacks: bool,
    /// Maximum Claude calls per user request.
    pub max_iterations: u32,
    /// Default per-tool execution timeout.
    pub tool_timeout: Duration,
    /// Passed through to the telemetry server ("now" for time windows).
    pub ops_reference_time: String,
    pub telemetry_server_path: PathBuf,
    /// The only directory the filesystem MCP server may read.
    pub filesystem_root: PathBuf,
    pub trace_file: PathBuf,
    pub log_level: String,
}

impl Config {
    /// Load `.env` (if present in `base_dir`), then read the process environment.
    pub fn load(base_dir: &Path) -> Result<Self, ConfigError> {
        load_dotenv(base_dir)?;
        Self::from_lookup(base_dir, |name| std::env::var(name).ok())
    }

    /// Build the configuration from any key/value source.
    /// Relative paths are resolved against `base_dir`.
    pub fn from_lookup(
        base_dir: &Path,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        // Treat empty / whitespace-only values the same as "not set".
        let get = |name: &str| {
            lookup(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };

        let api_key = match get("ANTHROPIC_API_KEY") {
            None => return Err(ConfigError::MissingApiKey),
            Some(k) if k == API_KEY_PLACEHOLDER => return Err(ConfigError::PlaceholderApiKey),
            Some(k) => ApiKey(k),
        };

        let effort = match get("ANTHROPIC_EFFORT") {
            None => Effort::Medium,
            Some(v) => v.parse().map_err(|reason| ConfigError::InvalidValue {
                name: "ANTHROPIC_EFFORT",
                value: v.clone(),
                reason,
            })?,
        };

        let fallbacks = match get("ANTHROPIC_FALLBACKS") {
            None => true,
            Some(v) => parse_on_off("ANTHROPIC_FALLBACKS", &v)?,
        };

        let max_iterations = parse_number(
            "MAX_ITERATIONS",
            get("MAX_ITERATIONS"),
            DEFAULT_MAX_ITERATIONS,
            1..=50,
        )?;

        let tool_timeout_secs = parse_number(
            "TOOL_TIMEOUT_SECS",
            get("TOOL_TIMEOUT_SECS"),
            DEFAULT_TOOL_TIMEOUT_SECS,
            1..=300,
        )?;

        let path = |name: &str, default: &str| {
            let raw = get(name).unwrap_or_else(|| default.to_string());
            resolve(base_dir, &raw)
        };

        Ok(Self {
            api_key,
            model: get("ANTHROPIC_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            effort,
            fallbacks,
            max_iterations,
            tool_timeout: Duration::from_secs(tool_timeout_secs),
            ops_reference_time: get("OPS_REFERENCE_TIME")
                .unwrap_or_else(|| "2026-11-20T12:00:00Z".to_string()),
            telemetry_server_path: path(
                "TELEMETRY_SERVER_PATH",
                r"target\debug\ops-telemetry-mcp.exe",
            ),
            filesystem_root: path("FILESYSTEM_ROOT", r"mock_data\workspace"),
            trace_file: path("TRACE_FILE", r"traces\agent_traces.jsonl"),
            log_level: get("LOG_LEVEL").unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string()),
        })
    }
}

/// Load `base_dir\.env` into the process environment if the file exists.
///
/// Only this exact file is read (no searching parent folders), and variables
/// that are already set in the real environment are NOT overwritten.
pub fn load_dotenv(base_dir: &Path) -> Result<(), ConfigError> {
    let path = base_dir.join(".env");
    if !path.exists() {
        return Ok(());
    }
    dotenvy::from_path(&path)
        .map(|_| ())
        // dotenvy's error text never contains variable values, only the problem location.
        .map_err(|e| ConfigError::EnvFile(e.to_string()))
}

fn parse_on_off(name: &'static str, value: &str) -> Result<bool, ConfigError> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        _ => Err(ConfigError::InvalidValue {
            name,
            value: value.to_string(),
            reason: "expected on or off".to_string(),
        }),
    }
}

fn parse_number<T>(
    name: &'static str,
    value: Option<String>,
    default: T,
    allowed: std::ops::RangeInclusive<T>,
) -> Result<T, ConfigError>
where
    T: FromStr + PartialOrd + fmt::Display + Copy,
{
    let Some(raw) = value else {
        return Ok(default);
    };
    let invalid = |reason: String| ConfigError::InvalidValue {
        name,
        value: raw.clone(),
        reason,
    };
    let n: T = raw
        .parse()
        .map_err(|_| invalid("expected a whole number".to_string()))?;
    if !allowed.contains(&n) {
        return Err(invalid(format!(
            "must be between {} and {}",
            allowed.start(),
            allowed.end()
        )));
    }
    Ok(n)
}

fn resolve(base_dir: &Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { base_dir.join(p) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const BASE: &str = r"C:\project";

    fn load(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(Path::new(BASE), |k| map.get(k).cloned())
    }

    #[test]
    fn defaults_are_applied_when_only_the_key_is_set() {
        let cfg = load(&[("ANTHROPIC_API_KEY", "sk-ant-test")]).unwrap();
        assert_eq!(cfg.model, "claude-opus-5-5");
        assert_eq!(cfg.effort, Effort::Medium);
        assert!(cfg.fallbacks);
        assert_eq!(cfg.max_iterations, 8);
        assert_eq!(cfg.tool_timeout, Duration::from_secs(10));
        assert_eq!(cfg.ops_reference_time, "2026-11-20T12:00:00Z");
        assert_eq!(
            cfg.filesystem_root,
            Path::new(BASE).join(r"mock_data\workspace")
        );
        assert_eq!(cfg.log_level, "warn");
    }

    #[test]
    fn missing_key_is_an_error() {
        assert_eq!(load(&[]).unwrap_err(), ConfigError::MissingApiKey);
    }

    #[test]
    fn blank_key_counts_as_missing() {
        let err = load(&[("ANTHROPIC_API_KEY", "   ")]).unwrap_err();
        assert_eq!(err, ConfigError::MissingApiKey);
    }

    #[test]
    fn placeholder_key_is_rejected() {
        let err = load(&[("ANTHROPIC_API_KEY", API_KEY_PLACEHOLDER)]).unwrap_err();
        assert_eq!(err, ConfigError::PlaceholderApiKey);
    }

    #[test]
    fn api_key_never_appears_in_debug_or_display() {
        let cfg = load(&[("ANTHROPIC_API_KEY", "sk-ant-SECRET123")]).unwrap();
        assert!(!format!("{cfg:?}").contains("SECRET123"));
        assert!(!format!("{}", cfg.api_key).contains("SECRET123"));
        assert_eq!(cfg.api_key.expose(), "sk-ant-SECRET123");
    }

    #[test]
    fn overrides_are_parsed() {
        let cfg = load(&[
            ("ANTHROPIC_API_KEY", "k"),
            ("ANTHROPIC_MODEL", "claude-sonnet-5-5"),
            ("ANTHROPIC_EFFORT", "HIGH"),
            ("ANTHROPIC_FALLBACKS", "off"),
            ("MAX_ITERATIONS", "3"),
            ("TOOL_TIMEOUT_SECS", "30"),
            ("FILESYSTEM_ROOT", r"D:\other\workspace"),
        ])
        .unwrap();
        assert_eq!(cfg.model, "claude-sonnet-5-5");
        assert_eq!(cfg.effort, Effort::High);
        assert!(!cfg.fallbacks);
        assert_eq!(cfg.max_iterations, 3);
        assert_eq!(cfg.tool_timeout, Duration::from_secs(30));
        assert_eq!(cfg.filesystem_root, PathBuf::from(r"D:\other\workspace"));
    }

    #[test]
    fn invalid_values_name_the_variable() {
        let cases = [
            ("ANTHROPIC_EFFORT", "extreme"),
            ("ANTHROPIC_FALLBACKS", "maybe"),
            ("MAX_ITERATIONS", "0"),
            ("MAX_ITERATIONS", "eight"),
            ("TOOL_TIMEOUT_SECS", "-5"),
        ];
        for (name, value) in cases {
            let err = load(&[("ANTHROPIC_API_KEY", "k"), (name, value)]).unwrap_err();
            match err {
                ConfigError::InvalidValue { name: n, .. } => assert_eq!(n, name),
                other => panic!("{name}={value}: unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn dotenv_file_is_loaded_and_missing_file_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        // No .env -> not an error.
        assert!(load_dotenv(dir.path()).is_ok());
        // A malformed .env -> a readable error, never a panic.
        std::fs::write(dir.path().join(".env"), "THIS LINE IS NOT VALID\n").unwrap();
        assert!(matches!(
            load_dotenv(dir.path()),
            Err(ConfigError::EnvFile(_))
        ));
    }
}
