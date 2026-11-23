//! Integration tests: the agent's MCP client against the REAL external
//! filesystem server (`node node_modules\@modelcontextprotocol\server-filesystem\...`).
//!
//! Requires Node.js 24+ and `npm ci` (both checked by scripts\setup.ps1).
//! No API key is needed.

// Test-only file: a panic in a helper is simply a failed test.
#![allow(clippy::unwrap_used)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agent_client::check::{file_uri, text_of};
use agent_client::config::McpSettings;
use agent_client::mcp::connection::{McpConnection, McpError, ServerLaunch};
use agent_client::mcp::filesystem::{self, path_args};
use rmcp::model::ResourceContents;

const T: Duration = Duration::from_secs(10);

fn project_root() -> PathBuf {
    filesystem::canonical(&Path::new(env!("CARGO_MANIFEST_DIR")).join("..")).unwrap()
}

fn settings() -> McpSettings {
    McpSettings::from_lookup(&project_root(), &|_: &str| None).unwrap()
}

fn workspace() -> PathBuf {
    filesystem::canonical(&settings().filesystem_root).unwrap()
}

async fn connect() -> McpConnection {
    filesystem::connect(&settings())
        .await
        .expect("filesystem server should start and verify")
}

async fn read(conn: &McpConnection, path: &str) -> rmcp::model::CallToolResult {
    conn.call_tool("read_text_file", path_args(path), T)
        .await
        .unwrap()
}

/// `tasklist` lists the PID while the process is alive.
fn process_alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

fn node_launch(name: &'static str, script: &str) -> ServerLaunch {
    ServerLaunch {
        name,
        program: PathBuf::from("node"),
        args: vec![OsString::from("-e"), OsString::from(script)],
        cwd: project_root(),
        env: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Startup, connection, discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn starts_connects_and_discovers_only_read_only_tools() {
    let conn = connect().await;
    assert!(conn.pid().is_some());
    assert!(
        conn.server_label()
            .starts_with("secure-filesystem-server 2.4.0"),
        "{}",
        conn.server_label()
    );

    let tools = conn.list_tools(T).await.unwrap();
    let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "directory_tree",
            "get_file_info",
            "list_allowed_directories",
            "list_directory",
            "list_directory_with_sizes",
            "read_file",
            "read_media_file",
            "read_multiple_files",
            "read_text_file",
            "search_files"
        ]
    );
    assert!(
        tools
            .iter()
            .all(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
    );
    conn.shutdown().await.unwrap();
}

#[tokio::test]
async fn resource_discovery_offers_the_file_template_and_lists_nothing() {
    let conn = connect().await;
    let templates = conn.list_resource_templates(T).await.unwrap();
    assert_eq!(templates.len(), 1);
    assert_eq!(templates[0].uri_template, "file://{+path}");
    // By design the server never enumerates the disk.
    assert!(conn.list_resources(T).await.unwrap().is_empty());
    conn.shutdown().await.unwrap();
}

#[tokio::test]
async fn the_only_allowed_directory_is_the_workspace() {
    let conn = connect().await;
    let r = conn
        .call_tool("list_allowed_directories", Default::default(), T)
        .await
        .unwrap();
    filesystem::verify_allowed_directories(&text_of(&r), &workspace()).unwrap();
    conn.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Reading the two workspace files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reads_release_notes_and_service_config() {
    let conn = connect().await;

    let notes = read(&conn, "RELEASE_NOTES.md").await;
    assert_ne!(notes.is_error, Some(true), "absent isError means success");
    let text = text_of(&notes);
    assert!(
        text.contains("3.2.0") && text.contains("Rollback plan"),
        "{text}"
    );

    let config = read(&conn, "service-config.yaml").await;
    assert!(text_of(&config).contains("batch_send_v2: true"));

    // Absolute Windows path (with spaces) works too.
    let absolute = workspace()
        .join("service-config.yaml")
        .display()
        .to_string();
    assert_eq!(text_of(&read(&conn, &absolute).await), text_of(&config));

    // And the same file through resources/read.
    let res = conn
        .read_resource(&file_uri(&workspace().join("RELEASE_NOTES.md")), T)
        .await
        .unwrap();
    assert!(
        matches!(&res.contents[0], ResourceContents::TextResourceContents { text, .. } if text.contains("3.2.0"))
    );

    // Directory listing shows exactly the two workspace files.
    let listing = conn
        .call_tool("list_directory", path_args("."), T)
        .await
        .unwrap();
    let listing = text_of(&listing);
    assert!(
        listing.contains("RELEASE_NOTES.md") && listing.contains("service-config.yaml"),
        "{listing}"
    );
    assert_eq!(
        listing.lines().filter(|l| !l.trim().is_empty()).count(),
        2,
        "{listing}"
    );
    conn.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Confinement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn access_outside_the_workspace_is_denied() {
    let conn = connect().await;
    let root = project_root();
    let attempts = [
        r"..\..\.env".to_string(),
        "../../.env".to_string(),
        root.join(".env").display().to_string(),
        root.join(".env.example").display().to_string(),
        root.join("Cargo.toml").display().to_string(),
        root.join("mock_data")
            .join("logs.json")
            .display()
            .to_string(),
        root.join("docs")
            .join("MOCK_DATA_SPEC.md")
            .display()
            .to_string(),
        r"C:\Windows\win.ini".to_string(),
        r"~\.ssh\id_rsa".to_string(),
        r"\\localhost\C$\Windows\win.ini".to_string(),
        r"\\?\C:\Windows\win.ini".to_string(),
        "RELEASE_NOTES.md::$DATA".to_string(),
        "NUL".to_string(),
    ];
    for path in &attempts {
        let r = read(&conn, path).await;
        assert_eq!(r.is_error, Some(true), "{path} must be refused");
        assert!(
            text_of(&r).contains("Access denied"),
            "{path}: {}",
            text_of(&r)
        );
    }

    // Directory tools are confined the same way.
    for (tool, args) in [
        ("list_directory", path_args("..")),
        ("directory_tree", path_args(root.display().to_string())),
        (
            "get_file_info",
            path_args(root.join(".env").display().to_string()),
        ),
    ] {
        let r = conn.call_tool(tool, args, T).await.unwrap();
        assert_eq!(r.is_error, Some(true), "{tool}");
        assert!(
            text_of(&r).contains("Access denied"),
            "{tool}: {}",
            text_of(&r)
        );
    }
    let mut search = path_args(".");
    search.insert("pattern".to_string(), "../**/.env".into());
    let r = conn.call_tool("search_files", search, T).await.unwrap();
    assert!(
        !text_of(&r).contains(".env"),
        "search must not find files outside: {}",
        text_of(&r)
    );

    // resources/read of an outside file is a JSON-RPC error.
    let err = conn
        .read_resource(&file_uri(&root.join("Cargo.toml")), T)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, McpError::Rejected { message, .. } if message.contains("Access denied")),
        "{err}"
    );
    conn.shutdown().await.unwrap();
}

#[tokio::test]
async fn write_tools_do_not_exist() {
    let conn = connect().await;
    for tool in [
        "write_file",
        "edit_file",
        "create_directory",
        "move_file",
        "copy_file",
        "append_file",
    ] {
        let mut args = path_args("pwned.txt");
        args.insert("content".to_string(), "x".into());
        let outcome = conn.call_tool(tool, args, T).await;
        let refused = match &outcome {
            Err(McpError::Rejected { message, .. }) => message.contains("not found"),
            Ok(r) => r.is_error == Some(true),
            Err(_) => false,
        };
        assert!(refused, "{tool}: {outcome:?}");
    }
    assert!(!workspace().join("pwned.txt").exists());
    conn.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_missing_file_is_an_error_result() {
    let conn = connect().await;
    let r = read(&conn, "DOES_NOT_EXIST.md").await;
    assert_eq!(r.is_error, Some(true));
    assert!(text_of(&r).contains("ENOENT"), "{}", text_of(&r));
    // The connection is still fine afterwards.
    assert_ne!(
        read(&conn, "RELEASE_NOTES.md").await.is_error,
        Some(true),
        "absent isError means success"
    );
    conn.shutdown().await.unwrap();
}

// ---------------------------------------------------------------------------
// Process lifecycle: shutdown, disconnection, startup failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shutdown_stops_the_node_process() {
    let conn = connect().await;
    let pid = conn.pid().unwrap();
    assert!(process_alive(pid));
    conn.shutdown().await.unwrap();
    // Give Windows a moment to reap the process.
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!process_alive(pid), "node process {pid} is still running");
}

#[tokio::test]
async fn a_killed_server_is_reported_as_disconnected_not_a_hang() {
    let conn = connect().await;
    let pid = conn.pid().unwrap();
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output()
        .unwrap();
    assert!(status.status.success(), "taskkill failed");

    let started = Instant::now();
    let err = conn
        .call_tool("read_text_file", path_args("RELEASE_NOTES.md"), T)
        .await
        .unwrap_err();
    assert!(matches!(err, McpError::Disconnected { .. }), "{err}");
    assert!(
        started.elapsed() < T,
        "must fail fast, took {:?}",
        started.elapsed()
    );
    // Further calls fail the same way.
    assert!(matches!(
        conn.list_tools(T).await.unwrap_err(),
        McpError::Disconnected { .. }
    ));
}

#[tokio::test]
async fn a_missing_node_executable_is_a_clear_spawn_error() {
    let mut s = settings();
    s.node_exe = PathBuf::from(r"C:\definitely\not\here\node.exe");
    let err = filesystem::connect(&s).await.err().unwrap();
    assert!(
        matches!(&err, McpError::Spawn { reason, .. } if reason.contains("not found")),
        "{err}"
    );
}

#[tokio::test]
async fn a_missing_server_package_is_explained() {
    let mut s = settings();
    s.filesystem_server_script = project_root().join("node_modules").join("nope.js");
    let err = filesystem::launch(&s).unwrap_err().to_string();
    assert!(err.contains("npm ci"), "{err}");
}

#[tokio::test]
async fn an_unsafe_workspace_is_refused_before_launch() {
    let mut s = settings();
    s.filesystem_root = project_root(); // the repository root holds .env, data\, ...
    let err = filesystem::launch(&s).unwrap_err().to_string();
    assert!(err.contains("refused for safety"), "{err}");

    s.filesystem_root = project_root().join("mock_data").join("missing-folder");
    assert!(filesystem::launch(&s).is_err());
}

#[tokio::test]
async fn a_program_that_is_not_an_mcp_server_fails_the_handshake_with_its_stderr() {
    let launch = node_launch(
        "broken",
        "process.stderr.write('boom: not an MCP server\\n'); process.exit(3)",
    );
    let err = McpConnection::start(&launch, T).await.err().unwrap();
    match &err {
        McpError::Handshake { stderr, .. } => {
            assert!(stderr.iter().any(|l| l.contains("boom")), "{err}")
        }
        other => panic!("expected a handshake error, got {other}"),
    }
}

#[tokio::test]
async fn a_server_that_never_answers_hits_the_startup_timeout() {
    let launch = node_launch("silent", "setTimeout(() => {}, 60000)");
    let started = Instant::now();
    let err = McpConnection::start(&launch, Duration::from_secs(2))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, McpError::StartupTimeout { secs: 2, .. }),
        "{err}"
    );
    assert!(started.elapsed() < Duration::from_secs(8));
}

/// The child must not inherit the agent's environment (which holds the API
/// key loaded from .env). The test process has many variables (CARGO_*, ...);
/// the child must see only the allow-listed system ones.
#[tokio::test]
async fn servers_start_with_a_scrubbed_environment() {
    let launch = node_launch(
        "env-probe",
        "process.stderr.write('ENV ' + Object.keys(process.env).join(',') + '\\n'); process.exit(0)",
    );
    let err = McpConnection::start(&launch, T).await.err().unwrap();
    // Precondition: the parent really has extra variables (cargo sets this one).
    assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
    let McpError::Handshake { stderr, .. } = &err else {
        panic!("unexpected {err}")
    };
    let line = stderr
        .iter()
        .find(|l| l.starts_with("ENV "))
        .expect("probe output");
    let keys: Vec<String> = line[4..]
        .split(',')
        .map(|k| k.to_ascii_uppercase())
        .collect();
    assert!(!keys.is_empty());
    let allowed = [
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
    for key in &keys {
        assert!(
            allowed.contains(&key.as_str()),
            "unexpected variable leaked to the child: {key}"
        );
    }
}
