//! Process-level checks of the compiled `ops-telemetry-mcp` binary
//! (no MCP client involved; see `mcp_protocol.rs` for protocol tests).

use std::process::Command;

/// With the stdio transport, stdout is the protocol channel, so the server
/// must never write anything else there. Logs belong on stderr.
/// `output()` gives the child an empty stdin, i.e. a client that disconnects
/// immediately: the server must shut down cleanly.
#[test]
fn exits_cleanly_when_the_client_disconnects_and_keeps_stdout_clean() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ops-telemetry-mcp"))
        .current_dir(dir.path())
        .env_remove("RUST_LOG")
        .env_remove("OPS_REFERENCE_TIME")
        .env("LOG_LEVEL", "debug")
        .output()
        .expect("failed to run ops-telemetry-mcp");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stderr: {stderr}");
    assert!(
        out.stdout.is_empty(),
        "stdout must stay empty, got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(stderr.contains("starting on stdio"), "stderr: {stderr}");
    assert!(
        stderr.contains("data folder not found"),
        "warns about the missing data: {stderr}"
    );
}

#[test]
fn an_invalid_reference_time_stops_startup_with_a_clear_message() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ops-telemetry-mcp"))
        .current_dir(dir.path())
        .env("OPS_REFERENCE_TIME", "yesterday")
        .output()
        .expect("failed to run ops-telemetry-mcp");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        stderr.contains("OPS_REFERENCE_TIME=`yesterday` is not valid"),
        "stderr: {stderr}"
    );
    assert!(out.stdout.is_empty());
}
