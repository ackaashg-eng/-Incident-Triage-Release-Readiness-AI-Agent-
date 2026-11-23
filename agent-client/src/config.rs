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
const DEFAULT_STARTUP_TIMEOUT_SECS: u64 = 20;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 300;
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

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
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The real key, for the HTTP header only. Never log the result.
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
    /// Ask for short progress notes between tool calls (`thinking.display: "updates"`).
    pub progress_updates: bool,
    /// `https://api.anthropic.com` unless overridden (tests use a local fake server).
    pub api_base_url: String,
    /// Timeout for one Messages API HTTP request.
    pub request_timeout: Duration,
    /// Maximum Claude calls per user request.
    pub max_iterations: u32,
    /// How to launch and talk to the two MCP servers.
    pub mcp: McpSettings,
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
        let lookup = &lookup as &dyn Fn(&str) -> Option<String>;
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

        let progress_updates = match get("ANTHROPIC_PROGRESS_UPDATES") {
            None => true,
            Some(v) => parse_on_off("ANTHROPIC_PROGRESS_UPDATES", &v)?,
        };

        let request_timeout_secs = parse_number(
            "ANTHROPIC_TIMEOUT_SECS",
            get("ANTHROPIC_TIMEOUT_SECS"),
            DEFAULT_REQUEST_TIMEOUT_SECS,
            10..=900,
        )?;

        let api_base_url = get("ANTHROPIC_BASE_URL")
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string();
        if !(api_base_url.starts_with("https://")
            || api_base_url.starts_with("http://127.0.0.1")
            || api_base_url.starts_with("http://localhost"))
        {
            return Err(ConfigError::InvalidValue {
                name: "ANTHROPIC_BASE_URL",
                value: api_base_url,
                reason: "must be an https:// URL (plain http only for localhost testing)"
                    .to_string(),
            });
        }

        let max_iterations = parse_number(
            "MAX_ITERATIONS",
            get("MAX_ITERATIONS"),
            DEFAULT_MAX_ITERATIONS,
            1..=50,
        )?;

        let mcp = McpSettings::from_lookup(base_dir, lookup)?;

        Ok(Self {
            api_key,
            model: get("ANTHROPIC_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string()),
            effort,
            fallbacks,
            progress_updates,
            api_base_url,
            request_timeout: Duration::from_secs(request_timeout_secs),
            max_iterations,
            mcp,
            trace_file: resolve(
                base_dir,
                &get("TRACE_FILE").unwrap_or_else(|| r"traces\agent_traces.jsonl".to_string()),
            ),
            log_level: log_level(lookup),
        })
    }
}

/// Settings for launching the MCP servers. They need no API key, so the
/// `check-mcp` command can use them on their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSettings {
    /// The project folder; servers are confined relative to it.
    pub project_root: PathBuf,
    /// Default timeout for one tool call / resource read.
    pub tool_timeout: Duration,
    /// How long a server may take to start and complete the MCP handshake.
    pub startup_timeout: Duration,
    /// Passed through to the telemetry server ("now" for time windows).
    pub ops_reference_time: String,
    pub telemetry_server_path: PathBuf,
    /// SQLite file the telemetry server stores tickets in.
    pub tickets_db: PathBuf,
    /// The Node.js executable (`node` = found on PATH).
    pub node_exe: PathBuf,
    /// Entry script of `@modelcontextprotocol/server-filesystem`.
    pub filesystem_server_script: PathBuf,
    /// The only directory the filesystem MCP server may read.
    pub filesystem_root: PathBuf,
}

impl McpSettings {
    /// Load `.env` (if present), then read the process environment.
    pub fn load(base_dir: &Path) -> Result<Self, ConfigError> {
        load_dotenv(base_dir)?;
        Self::from_lookup(base_dir, &|name: &str| std::env::var(name).ok())
    }

    pub fn from_lookup(
        base_dir: &Path,
        lookup: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let get = |name: &str| {
            lookup(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let path = |name: &str, default: &str| {
            resolve(base_dir, &get(name).unwrap_or_else(|| default.to_string()))
        };
        let tool_timeout_secs = parse_number(
            "TOOL_TIMEOUT_SECS",
            get("TOOL_TIMEOUT_SECS"),
            DEFAULT_TOOL_TIMEOUT_SECS,
            1..=300,
        )?;
        let startup_timeout_secs = parse_number(
            "MCP_STARTUP_TIMEOUT_SECS",
            get("MCP_STARTUP_TIMEOUT_SECS"),
            DEFAULT_STARTUP_TIMEOUT_SECS,
            1..=120,
        )?;

        Ok(Self {
            project_root: base_dir.to_path_buf(),
            tool_timeout: Duration::from_secs(tool_timeout_secs),
            startup_timeout: Duration::from_secs(startup_timeout_secs),
            ops_reference_time: get("OPS_REFERENCE_TIME")
                .unwrap_or_else(|| "2026-11-20T12:00:00Z".to_string()),
            telemetry_server_path: path(
                "TELEMETRY_SERVER_PATH",
                r"target\debug\ops-telemetry-mcp.exe",
            ),
            tickets_db: path("TICKETS_DB", r"data\tickets.db"),
            // A bare program name is looked up on PATH, so it is not joined to base_dir.
            node_exe: PathBuf::from(get("NODE_EXE").unwrap_or_else(|| "node".to_string())),
            filesystem_server_script: path(
                "FILESYSTEM_SERVER_SCRIPT",
                r"node_modules\@modelcontextprotocol\server-filesystem\dist\index.js",
            ),
            filesystem_root: path("FILESYSTEM_ROOT", r"mock_data\workspace"),
        })
    }
}

/// `LOG_LEVEL` (default `warn`); shared by the agent and `check-mcp`.
pub fn log_level(lookup: &dyn Fn(&str) -> Option<String>) -> String {
    lookup("LOG_LEVEL")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_LEVEL.to_string())
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
        assert!(cfg.progress_updates);
        assert_eq!(cfg.api_base_url, "https://api.anthropic.com");
        assert_eq!(cfg.request_timeout, Duration::from_secs(300));
        assert_eq!(cfg.max_iterations, 8);
        assert_eq!(cfg.mcp.tool_timeout, Duration::from_secs(10));
        assert_eq!(cfg.mcp.startup_timeout, Duration::from_secs(20));
        assert_eq!(cfg.mcp.ops_reference_time, "2026-11-20T12:00:00Z");
        assert_eq!(cfg.mcp.node_exe, PathBuf::from("node"));
        assert_eq!(
            cfg.mcp.filesystem_server_script,
            Path::new(BASE)
                .join(r"node_modules\@modelcontextprotocol\server-filesystem\dist\index.js")
        );
        assert_eq!(
            cfg.mcp.filesystem_root,
            Path::new(BASE).join(r"mock_data\workspace")
        );
        assert_eq!(cfg.log_level, "warn");
    }

    #[test]
    fn mcp_settings_need_no_api_key() {
        let s = McpSettings::from_lookup(Path::new(BASE), &|k: &str| {
            (k == "NODE_EXE").then(|| r"C:\tools\node.exe".to_string())
        })
        .unwrap();
        assert_eq!(s.project_root, PathBuf::from(BASE));
        assert_eq!(s.node_exe, PathBuf::from(r"C:\tools\node.exe"));
        assert_eq!(
            s.filesystem_root,
            Path::new(BASE).join(r"mock_data\workspace")
        );
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
        assert_eq!(cfg.mcp.tool_timeout, Duration::from_secs(30));
        assert_eq!(
            cfg.mcp.filesystem_root,
            PathBuf::from(r"D:\other\workspace")
        );
    }

    #[test]
    fn invalid_values_name_the_variable() {
        let cases = [
            ("ANTHROPIC_EFFORT", "extreme"),
            ("ANTHROPIC_FALLBACKS", "maybe"),
            ("MAX_ITERATIONS", "0"),
            ("MAX_ITERATIONS", "eight"),
            ("TOOL_TIMEOUT_SECS", "-5"),
            ("MCP_STARTUP_TIMEOUT_SECS", "0"),
            ("ANTHROPIC_TIMEOUT_SECS", "5"),
            ("ANTHROPIC_PROGRESS_UPDATES", "sometimes"),
            ("ANTHROPIC_BASE_URL", "http://evil.example.com"),
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
