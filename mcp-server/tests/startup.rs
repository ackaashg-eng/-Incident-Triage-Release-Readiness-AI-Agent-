//! Checks of the compiled `ops-telemetry-mcp` binary.

use std::process::Command;

/// With the stdio transport, stdout is the protocol channel, so the server
/// must never write anything else there. Logs belong on stderr.
#[test]
fn starts_and_keeps_stdout_clean() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ops-telemetry-mcp"))
        .current_dir(dir.path())
        .env_remove("RUST_LOG")
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
    assert!(stderr.contains("foundation ready"), "stderr: {stderr}");
}
