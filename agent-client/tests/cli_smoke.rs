//! End-to-end checks of the compiled `agent-client` binary.
//! Each test runs it inside a fresh temporary folder, so the real project
//! `.env` is never read and no API key is needed.

use std::process::{Command, Output};

fn run_in(dir: &std::path::Path, extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_agent-client"));
    cmd.current_dir(dir);
    // Make the tests independent of anything set on the developer's machine.
    for var in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_MODEL",
        "RUST_LOG",
        "LOG_LEVEL",
    ] {
        cmd.env_remove(var);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run agent-client")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn missing_key_gives_a_friendly_error() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_in(dir.path(), &[]);

    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("ANTHROPIC_API_KEY is not set"),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("setup.ps1"), "stderr: {stderr}");
}

#[test]
fn key_from_dotenv_is_used_but_never_printed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".env"),
        "ANTHROPIC_API_KEY=sk-ant-test-SECRET123\nLOG_LEVEL=debug\n",
    )
    .unwrap();

    let out = run_in(dir.path(), &[]);

    assert!(out.status.success(), "stderr: {}", text(&out.stderr));
    let stdout = text(&out.stdout);
    let stderr = text(&out.stderr);
    assert!(
        stdout.contains("API key            : loaded (***)"),
        "stdout: {stdout}"
    );
    assert!(!stdout.contains("SECRET123"), "key leaked to stdout");
    assert!(
        !stderr.contains("SECRET123"),
        "key leaked to stderr (debug logging is on)"
    );
}

#[test]
fn real_environment_overrides_dotenv() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join(".env"),
        "ANTHROPIC_API_KEY=k\nANTHROPIC_MODEL=model-from-dotenv\n",
    )
    .unwrap();

    let out = run_in(dir.path(), &[("ANTHROPIC_MODEL", "model-from-env")]);

    let stdout = text(&out.stdout);
    assert!(stdout.contains("model-from-env"), "stdout: {stdout}");
    assert!(!stdout.contains("model-from-dotenv"));
}

/// The committed `.env.example` must parse, must contain only the placeholder
/// key, and its documented defaults must be accepted by the config loader.
#[test]
fn env_example_parses_and_contains_only_the_placeholder() {
    let example = concat!(env!("CARGO_MANIFEST_DIR"), "/../.env.example");
    let content = std::fs::read_to_string(example).unwrap();
    assert!(
        !content.contains("sk-ant-"),
        ".env.example must not contain a real key"
    );

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".env"), &content).unwrap();
    let out = run_in(dir.path(), &[]);

    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(
        stderr.contains("still contains the placeholder"),
        "stderr: {stderr}"
    );
}

#[test]
fn invalid_setting_is_reported_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let out = run_in(
        dir.path(),
        &[("ANTHROPIC_API_KEY", "k"), ("MAX_ITERATIONS", "0")],
    );

    assert!(!out.status.success());
    let stderr = text(&out.stderr);
    assert!(stderr.contains("MAX_ITERATIONS"), "stderr: {stderr}");
    assert!(stderr.contains("between 1 and 50"), "stderr: {stderr}");
}
