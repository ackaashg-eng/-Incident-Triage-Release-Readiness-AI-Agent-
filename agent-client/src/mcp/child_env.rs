//! The environment given to MCP server child processes.
//!
//! A child process normally inherits *every* environment variable of its
//! parent. Ours holds `ANTHROPIC_API_KEY` (loaded from `.env`), so an
//! inherited environment would hand the key to third-party code (the Node
//! filesystem server and its dependencies). Instead, each server starts with an
//! **empty** environment plus only the variables on this allow-list, which
//! Windows and Node need to run, and the few settings we pass on purpose.

use std::ffi::OsString;

/// Variables a Windows process (and Node.js) needs. Nothing secret.
const INHERITED: [&str; 17] = [
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
];

/// Keeps only allow-listed variables. Names are compared case-insensitively,
/// because Windows treats `Path` and `PATH` as the same variable.
pub fn filter(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    vars.into_iter()
        .filter(|(name, _)| {
            name.to_str().is_some_and(|n| {
                INHERITED
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(n))
            })
        })
        .collect()
}

/// The allow-listed part of this process's environment.
pub fn inherited() -> Vec<(OsString, OsString)> {
    filter(std::env::vars_os())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(k: &str, v: &str) -> (OsString, OsString) {
        (k.into(), v.into())
    }

    #[test]
    fn secrets_are_dropped_and_system_variables_kept() {
        let kept = filter([
            pair("ANTHROPIC_API_KEY", "sk-ant-SECRET"),
            pair("Path", r"C:\Windows"),
            pair("SystemRoot", r"C:\Windows"),
            pair("AWS_SECRET_ACCESS_KEY", "x"),
            pair("GITHUB_TOKEN", "x"),
            pair("TEMP", r"C:\Temp"),
            pair("LOG_LEVEL", "debug"),
        ]);
        let names: Vec<String> = kept
            .iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["Path", "SystemRoot", "TEMP"]);
    }
}
