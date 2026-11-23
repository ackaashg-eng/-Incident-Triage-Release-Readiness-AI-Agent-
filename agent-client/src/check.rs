//! `agent-client check-mcp`: starts both MCP servers, exercises them the way
//! the agent will, prints a report and shuts them down. No API key needed.
//!
//! Every line is an expectation: `[ok]` when the real behaviour matched it,
//! `[FAIL]` when not. The command exits with an error if anything failed.

use std::path::Path;
use std::time::Duration;

use rmcp::model::{CallToolResult, ResourceContents};

use crate::config::McpSettings;
use crate::mcp::connection::{McpConnection, McpError};
use crate::mcp::{filesystem, telemetry};

#[derive(Default)]
struct Report {
    failures: usize,
}

impl Report {
    fn check(&mut self, ok: bool, what: &str, detail: impl AsRef<str>) {
        if !ok {
            self.failures += 1;
        }
        let mark = if ok { "ok  " } else { "FAIL" };
        println!("  [{mark}] {what}: {}", detail.as_ref());
    }
}

/// Returns `Ok(true)` when every check passed.
pub async fn run(settings: &McpSettings) -> anyhow::Result<bool> {
    let mut report = Report::default();
    println!("MCP connection check (Phase 4)");
    println!("project folder: {}", settings.project_root.display());

    println!("\n[files] external filesystem server");
    match filesystem::launch(settings) {
        Ok(launch) => println!("  command: {}", launch.display()),
        Err(err) => report.check(false, "launch configuration", err.to_string()),
    }
    match filesystem::connect(settings).await {
        Ok(conn) => check_filesystem(&mut report, conn, settings).await,
        Err(err) => report.check(false, "start + verify", err.to_string()),
    }

    println!("\n[telemetry] ops-telemetry-mcp");
    match telemetry::connect(settings).await {
        Ok(conn) => check_telemetry(&mut report, conn, settings.tool_timeout).await,
        Err(err) => report.check(false, "start", err.to_string()),
    }

    println!();
    if report.failures == 0 {
        println!("All checks passed.");
    } else {
        println!("{} check(s) FAILED.", report.failures);
    }
    Ok(report.failures == 0)
}

async fn check_filesystem(report: &mut Report, conn: McpConnection, settings: &McpSettings) {
    let t = settings.tool_timeout;
    report.check(
        true,
        "connected",
        format!(
            "pid {:?}, {}, protocol {}",
            conn.pid(),
            conn.server_label(),
            conn.protocol_version()
        ),
    );
    report.check(
        true,
        "verified",
        "every tool is read-only; the only allowed directory is the workspace",
    );

    match conn.list_tools(t).await {
        Ok(tools) => {
            let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
            names.sort_unstable();
            let has_writers = names.iter().any(|n| {
                [
                    "write_file",
                    "edit_file",
                    "move_file",
                    "copy_file",
                    "append_file",
                    "create_directory",
                ]
                .contains(n)
            });
            report.check(
                !has_writers,
                &format!("tools ({})", names.len()),
                names.join(", "),
            );
        }
        Err(err) => report.check(false, "tools/list", err.to_string()),
    }
    match conn.list_resource_templates(t).await {
        Ok(tpl) => {
            let uris: Vec<&str> = tpl.iter().map(|r| r.uri_template.as_str()).collect();
            report.check(true, "resource templates", uris.join(", "));
        }
        Err(err) => report.check(false, "resources/templates/list", err.to_string()),
    }

    for (file, must_contain) in [
        ("RELEASE_NOTES.md", "3.2.0"),
        ("service-config.yaml", "batch_send_v2"),
    ] {
        match read_text(&conn, file, t).await {
            Ok(text) => report.check(
                text.contains(must_contain),
                &format!("read {file}"),
                format!(
                    "{} lines, {} bytes; first line: {}",
                    text.lines().count(),
                    text.len(),
                    first_line(&text)
                ),
            ),
            Err(err) => report.check(false, &format!("read {file}"), err),
        }
    }

    let workspace = filesystem::canonical(&settings.filesystem_root).unwrap_or_default();
    let uri = file_uri(&workspace.join("RELEASE_NOTES.md"));
    match conn.read_resource(&uri, t).await {
        Ok(res) => {
            let ok = res.contents.iter().any(|c| matches!(c, ResourceContents::TextResourceContents { text, .. } if text.contains("3.2.0")));
            report.check(ok, "resources/read", uri);
        }
        Err(err) => report.check(false, "resources/read", err.to_string()),
    }

    let root = &settings.project_root;
    let outside = [
        r"..\..\.env".to_string(),
        root.join(".env").display().to_string(),
        root.join("Cargo.toml").display().to_string(),
        root.join("mock_data")
            .join("logs.json")
            .display()
            .to_string(),
        r"C:\Windows\win.ini".to_string(),
        r"~\.ssh\id_rsa".to_string(),
        "RELEASE_NOTES.md::$DATA".to_string(),
        r"\\localhost\C$\Windows\win.ini".to_string(),
    ];
    for path in outside {
        let outcome = conn
            .call_tool("read_text_file", filesystem::path_args(path.clone()), t)
            .await;
        let (ok, detail) = expect_denied(&outcome);
        report.check(ok, &format!("outside access `{path}`"), detail);
    }

    let outcome = conn
        .call_tool(
            "read_text_file",
            filesystem::path_args("DOES_NOT_EXIST.md"),
            t,
        )
        .await;
    let (ok, detail) = match &outcome {
        Ok(r) if r.is_error == Some(true) => {
            (true, format!("error result: {}", short(&text_of(r))))
        }
        Ok(_) => (false, "unexpectedly succeeded".to_string()),
        Err(e) => (false, e.to_string()),
    };
    report.check(ok, "missing file", detail);

    let mut args = filesystem::path_args("pwned.txt");
    args.insert("content".to_string(), "x".into());
    let outcome = conn.call_tool("write_file", args, t).await;
    let written = workspace.join("pwned.txt").exists();
    let refused = !written && !matches!(&outcome, Ok(r) if r.is_error != Some(true));
    report.check(
        refused,
        "write attempt (write_file)",
        match &outcome {
            Ok(r) => format!("refused: {}", short(&text_of(r))),
            Err(e) => format!("refused: {e}"),
        },
    );

    let pid = conn.pid();
    match conn.shutdown().await {
        Ok(()) => report.check(true, "shutdown", format!("process {pid:?} stopped")),
        Err(err) => report.check(false, "shutdown", err.to_string()),
    }
}

async fn check_telemetry(report: &mut Report, conn: McpConnection, t: Duration) {
    report.check(
        true,
        "connected",
        format!(
            "pid {:?}, {}, protocol {}",
            conn.pid(),
            conn.server_label(),
            conn.protocol_version()
        ),
    );
    match conn.list_tools(t).await {
        Ok(tools) => {
            let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
            names.sort_unstable();
            report.check(names.len() == 3, "tools", names.join(", "));
        }
        Err(err) => report.check(false, "tools/list", err.to_string()),
    }
    match conn.list_resources(t).await {
        Ok(res) => report.check(res.len() == 4, "resources", format!("{} listed", res.len())),
        Err(err) => report.check(false, "resources/list", err.to_string()),
    }
    let pid = conn.pid();
    match conn.shutdown().await {
        Ok(()) => report.check(true, "shutdown", format!("process {pid:?} stopped")),
        Err(err) => report.check(false, "shutdown", err.to_string()),
    }
}

async fn read_text(conn: &McpConnection, path: &str, t: Duration) -> Result<String, String> {
    match conn
        .call_tool("read_text_file", filesystem::path_args(path), t)
        .await
    {
        Ok(r) if r.is_error != Some(true) => Ok(text_of(&r)),
        Ok(r) => Err(format!("error result: {}", text_of(&r))),
        Err(e) => Err(e.to_string()),
    }
}

/// An outside path must be refused, either as an error result or a JSON-RPC
/// error, and the message must say access was denied.
fn expect_denied(outcome: &Result<CallToolResult, McpError>) -> (bool, String) {
    match outcome {
        Ok(r) if r.is_error == Some(true) => {
            let text = text_of(r);
            (
                text.contains("Access denied"),
                format!("denied: {}", short(&text)),
            )
        }
        Ok(r) => (
            false,
            format!("NOT denied, returned: {}", short(&text_of(r))),
        ),
        Err(McpError::Rejected { message, .. }) => (
            message.contains("Access denied"),
            format!("denied: {}", short(message)),
        ),
        Err(e) => (false, e.to_string()),
    }
}

pub fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `file:///C:/a%20b/c.md`: forward slashes, spaces percent-encoded.
pub fn file_uri(path: &Path) -> String {
    format!(
        "file:///{}",
        path.display()
            .to_string()
            .replace('\\', "/")
            .replace(' ', "%20")
    )
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn short(text: &str) -> String {
    let one_line = text.replace(['\r', '\n'], " ");
    if one_line.chars().count() > 110 {
        format!("{}…", one_line.chars().take(110).collect::<String>())
    } else {
        one_line
    }
}
