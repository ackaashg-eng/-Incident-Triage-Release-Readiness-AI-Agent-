//! The external filesystem MCP server (`@modelcontextprotocol/server-filesystem`).
//!
//! We do not re-implement file reading: we launch the official reference
//! server as an independent component and only *configure* and *verify* it.
//!
//! Defence in depth, in order:
//! 1. **Before launch** (`launch`): the workspace must exist, must not be the
//!    project root or contain it, and must not contain secret-looking files
//!    (`.env*`, keys, databases) or links that could point elsewhere.
//! 2. **Launch flags**: `--read-only` (the server never registers write tools),
//!    `--confine-roots` (a client's MCP Roots can never widen access), and the
//!    workspace as the *only* allowed directory. Node runs with a scrubbed
//!    environment (no API key; no `NODE_OPTIONS`).
//! 3. **After connecting** (`connect`): every tool must be annotated
//!    `readOnlyHint: true`, and `list_allowed_directories` must report exactly
//!    the workspace. Otherwise the connection is closed and refused.
//!
//! File contents returned by this server are untrusted data (see Phase 6).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rmcp::model::Tool;

use super::connection::{McpConnection, McpError, ServerLaunch};
use crate::config::McpSettings;

pub const SERVER_NAME: &str = "files";

/// File-name patterns that must never be inside the shared workspace.
const SECRET_SUFFIXES: [&str; 7] = [".pem", ".key", ".pfx", ".p12", ".db", ".sqlite", ".kdbx"];
const MAX_SCAN_ENTRIES: usize = 10_000;

/// Builds the launch command after checking the workspace (step 1 + 2).
pub fn launch(settings: &McpSettings) -> Result<ServerLaunch, McpError> {
    let script = &settings.filesystem_server_script;
    if !script.is_file() {
        return Err(McpError::Spawn {
            server: SERVER_NAME,
            program: script.display().to_string(),
            reason: "the filesystem MCP server is not installed; run `npm ci` (or scripts\\setup.ps1) in the project folder".to_string(),
        });
    }
    let workspace = check_workspace(&settings.project_root, &settings.filesystem_root)?;

    Ok(ServerLaunch {
        name: SERVER_NAME,
        program: settings.node_exe.clone(),
        args: vec![
            script.as_os_str().to_owned(),
            "--read-only".into(),
            "--confine-roots".into(),
            // Everything after `--` is a directory, never an option.
            "--".into(),
            workspace.as_os_str().to_owned(),
        ],
        // Relative paths in tool calls resolve against the allowed directory,
        // but give the process a harmless working directory as well.
        cwd: workspace,
        env: Vec::new(),
    })
}

/// Launches, connects and verifies the filesystem server (step 3).
pub async fn connect(settings: &McpSettings) -> Result<McpConnection, McpError> {
    let launch = launch(settings)?;
    let workspace = launch.cwd.clone();
    let conn = McpConnection::start(&launch, settings.startup_timeout).await?;
    match verify(&conn, &workspace, settings.tool_timeout).await {
        Ok(()) => Ok(conn),
        Err(err) => {
            let _ = conn.shutdown().await;
            Err(err)
        }
    }
}

async fn verify(conn: &McpConnection, workspace: &Path, timeout: Duration) -> Result<(), McpError> {
    let tools = conn.list_tools(timeout).await?;
    verify_read_only(&tools)?;

    let result = conn
        .call_tool("list_allowed_directories", Default::default(), timeout)
        .await?;
    let text = result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    verify_allowed_directories(&text, workspace)
}

/// Every tool must promise not to modify anything. Fail closed otherwise.
pub fn verify_read_only(tools: &[Tool]) -> Result<(), McpError> {
    let writable: Vec<&str> = tools
        .iter()
        .filter(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) != Some(true))
        .map(|t| t.name.as_ref())
        .collect();
    if tools.is_empty() {
        return Err(unsafe_err("the server offered no tools at all".to_string()));
    }
    if writable.is_empty() {
        Ok(())
    } else {
        Err(unsafe_err(format!(
            "tools not marked read-only were offered: {}. Is --read-only missing?",
            writable.join(", ")
        )))
    }
}

/// `list_allowed_directories` must name the workspace and nothing else.
pub fn verify_allowed_directories(text: &str, workspace: &Path) -> Result<(), McpError> {
    let expected = normalise(workspace);
    let listed: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| l.contains(":\\") || l.starts_with("\\\\") || l.starts_with('/'))
        .map(|l| normalise(Path::new(l)))
        .collect();
    if listed.len() == 1 && listed[0] == expected {
        Ok(())
    } else {
        Err(unsafe_err(format!(
            "expected exactly one allowed directory ({}), the server reported: {}",
            workspace.display(),
            text.replace('\n', " | ")
        )))
    }
}

/// Checks the workspace and returns its canonical path (without the `\\?\` prefix).
pub fn check_workspace(project_root: &Path, workspace: &Path) -> Result<PathBuf, McpError> {
    let workspace = canonical(workspace).map_err(|e| {
        unsafe_err(format!(
            "the workspace folder {} cannot be used: {e}",
            workspace.display()
        ))
    })?;
    if !workspace.is_dir() {
        return Err(unsafe_err(format!(
            "{} is not a folder",
            workspace.display()
        )));
    }
    let root = canonical(project_root)
        .map_err(|e| unsafe_err(format!("the project folder cannot be resolved: {e}")))?;
    if root.starts_with(&workspace) {
        return Err(unsafe_err(format!(
            "the workspace {} is the project folder or contains it; it must be a dedicated sub-folder such as mock_data\\workspace",
            workspace.display()
        )));
    }
    if workspace.parent().is_none() {
        return Err(unsafe_err(
            "a drive root cannot be the workspace".to_string(),
        ));
    }
    let findings = scan_for_sensitive_entries(&workspace)?;
    if !findings.is_empty() {
        return Err(unsafe_err(format!(
            "the workspace contains entries that must not be shared with the agent: {}",
            findings.join(", ")
        )));
    }
    Ok(workspace)
}

/// Looks for secret-looking files and for links (symlinks/junctions) that
/// could lead outside the workspace. Links are not followed.
fn scan_for_sensitive_entries(workspace: &Path) -> Result<Vec<String>, McpError> {
    let mut findings = Vec::new();
    let mut pending = vec![workspace.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = pending.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| unsafe_err(format!("cannot scan {}: {e}", dir.display())))?;
        for entry in entries {
            seen += 1;
            if seen > MAX_SCAN_ENTRIES {
                return Err(unsafe_err(format!(
                    "the workspace has more than {MAX_SCAN_ENTRIES} entries; use a small dedicated folder"
                )));
            }
            let entry =
                entry.map_err(|e| unsafe_err(format!("cannot scan {}: {e}", dir.display())))?;
            let path = entry.path();
            let shown = path
                .strip_prefix(workspace)
                .unwrap_or(&path)
                .display()
                .to_string();
            let meta = std::fs::symlink_metadata(&path)
                .map_err(|e| unsafe_err(format!("cannot inspect {shown}: {e}")))?;
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if meta.file_type().is_symlink() || is_reparse_point(&meta) {
                findings.push(format!("{shown} (link)"));
            } else if meta.is_dir() {
                pending.push(path);
            } else if name.starts_with(".env") || SECRET_SUFFIXES.iter().any(|s| name.ends_with(s))
            {
                findings.push(shown);
            }
        }
    }
    findings.sort();
    Ok(findings)
}

#[cfg(windows)]
fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

/// `std::fs::canonicalize` on Windows returns `\\?\C:\...`. Node accepts that
/// form, but the plain `C:\...` form is clearer in logs and comparisons.
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    let full = std::fs::canonicalize(path)?;
    let text = full.as_os_str().to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => Ok(PathBuf::from(rest)),
        _ => Ok(full),
    }
}

/// Lower-case, no trailing separator: Windows paths compare case-insensitively.
fn normalise(path: &Path) -> String {
    let s = path.as_os_str().to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.trim_end_matches(['\\', '/']).to_lowercase()
}

fn unsafe_err(reason: String) -> McpError {
    McpError::Unsafe {
        server: SERVER_NAME,
        reason,
    }
}

/// Builds `{"path": ...}` tool arguments.
pub fn path_args(path: impl Into<OsString>) -> rmcp::model::JsonObject {
    let mut args = rmcp::model::JsonObject::new();
    args.insert(
        "path".to_string(),
        serde_json::Value::String(path.into().to_string_lossy().into_owned()),
    );
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ToolAnnotations;
    use std::sync::Arc;

    fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let ws = root.path().join("mock_data").join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("RELEASE_NOTES.md"), "# notes").unwrap();
        let root_path = root.path().to_path_buf();
        (root, root_path, ws)
    }

    #[test]
    fn a_clean_workspace_is_accepted_and_canonicalised() {
        let (_tmp, root, ws) = project();
        let checked = check_workspace(&root, &ws).unwrap();
        assert!(checked.is_absolute());
        assert!(!checked.to_string_lossy().starts_with(r"\\?\"));
        assert_eq!(normalise(&checked), normalise(&canonical(&ws).unwrap()));
    }

    #[test]
    fn the_project_root_or_a_parent_is_refused() {
        let (_tmp, root, _ws) = project();
        assert!(matches!(
            check_workspace(&root, &root),
            Err(McpError::Unsafe { .. })
        ));
        assert!(matches!(
            check_workspace(&root, root.parent().unwrap()),
            Err(McpError::Unsafe { .. })
        ));
    }

    #[test]
    fn a_missing_workspace_is_refused() {
        let (_tmp, root, ws) = project();
        assert!(check_workspace(&root, &ws.join("nope")).is_err());
    }

    #[test]
    fn secret_looking_files_are_refused() {
        for name in [
            ".env",
            ".env.local",
            "id.pem",
            "nested/tickets.db",
            "server.KEY",
        ] {
            let (_tmp, root, ws) = project();
            let file = ws.join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "x").unwrap();
            let err = check_workspace(&root, &ws).unwrap_err().to_string();
            assert!(err.contains("must not be shared"), "{name}: {err}");
        }
    }

    fn tool(name: &str, read_only: Option<bool>) -> Tool {
        let mut t = Tool::new(
            name.to_string(),
            "d",
            Arc::new(rmcp::model::JsonObject::new()),
        );
        t.annotations = read_only.map(|r| ToolAnnotations::new().read_only(r));
        t
    }

    #[test]
    fn read_only_verification_fails_closed() {
        assert!(verify_read_only(&[tool("read_text_file", Some(true))]).is_ok());
        for bad in [Some(false), None] {
            let err =
                verify_read_only(&[tool("read_text_file", Some(true)), tool("write_file", bad)])
                    .unwrap_err()
                    .to_string();
            assert!(err.contains("write_file"), "{err}");
        }
        assert!(verify_read_only(&[]).is_err());
    }

    #[test]
    fn allowed_directories_must_be_exactly_the_workspace() {
        let ws = Path::new(r"C:\Proj\mock_data\workspace");
        assert!(
            verify_allowed_directories("Allowed directories:\nC:\\proj\\mock_data\\workspace", ws)
                .is_ok()
        );
        assert!(verify_allowed_directories("Allowed directories:\nC:\\Proj", ws).is_err());
        assert!(
            verify_allowed_directories(
                "Allowed directories:\nC:\\Proj\\mock_data\\workspace\nC:\\Users",
                ws
            )
            .is_err()
        );
        assert!(verify_allowed_directories("Allowed directories:\n", ws).is_err());
    }
}
