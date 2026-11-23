//! MCP-level integration tests.
//!
//! Each test starts the compiled `ops-telemetry-mcp` executable as a child
//! process and talks to it with the official rmcp *client* over stdio,
//! exactly as the agent will in Phase 5. Nothing here calls Rust functions of
//! the server directly: everything goes through real JSON-RPC messages.

// Test-only file: a panic in a helper is simply a failed test.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{ServiceError, ServiceExt};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, ()>;

fn seed_data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("mock_data")
}

/// Starts the server binary with the given data folder and ticket database.
async fn start(data_dir: &Path, db: &Path) -> Client {
    let command =
        tokio::process::Command::new(env!("CARGO_BIN_EXE_ops-telemetry-mcp")).configure(|cmd| {
            cmd.env("DATA_DIR", data_dir)
                .env("TICKETS_DB", db)
                .env("OPS_REFERENCE_TIME", "2026-11-20T12:00:00Z")
                .env("LOG_LEVEL", "error")
                .env_remove("RUST_LOG");
        });
    let (transport, _stderr) = TokioChildProcess::builder(command)
        .stderr(Stdio::null())
        .spawn()
        .expect("server process should start");
    ().serve(transport).await.expect("MCP session should start")
}

async fn call(
    client: &Client,
    tool: &'static str,
    args: Value,
) -> Result<CallToolResult, ServiceError> {
    let args = args
        .as_object()
        .expect("arguments must be a JSON object")
        .clone();
    client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(args))
        .await
}

/// Calls a tool that must succeed and returns its structured content.
async fn ok(client: &Client, tool: &'static str, args: Value) -> Value {
    let result = call(client, tool, args).await.expect("no protocol error");
    assert_eq!(
        result.is_error,
        Some(false),
        "expected success, got {:?}",
        result.structured_content
    );
    result.structured_content.expect("structured content")
}

/// Calls a tool that must fail at tool level and returns the `error` object.
async fn tool_error(client: &Client, tool: &'static str, args: Value) -> Value {
    let result = call(client, tool, args).await.expect("no protocol error");
    assert_eq!(result.is_error, Some(true), "expected an error result");
    let body = result.structured_content.expect("structured error body");
    // The text content mirrors the structured body for clients that only read text.
    let text = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), body);
    body["error"].clone()
}

async fn read(client: &Client, uri: &str) -> Result<String, ServiceError> {
    let result = client
        .read_resource(ReadResourceRequestParams::new(uri))
        .await?;
    match &result.contents[0] {
        ResourceContents::TextResourceContents {
            text,
            mime_type,
            uri: returned,
            ..
        } => {
            assert_eq!(returned, uri);
            assert_eq!(mime_type.as_deref(), Some("text/markdown"));
            Ok(text.clone())
        }
        other => panic!("expected text contents, got {other:?}"),
    }
}

fn ticket_args(title: &str, severity: &str, assignee: &str) -> Value {
    json!({
        "title": title,
        "severity": severity,
        "summary": "Checkout failures since the 2.14.0 deploy at 09:05 UTC (log-0029, dep-1048).",
        "assignee": assignee
    })
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lists_three_tools_with_strict_schemas_and_annotations() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    let mut tools = client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "create_incident_ticket",
            "get_deployment_history",
            "search_logs"
        ]
    );

    for tool in &tools {
        let schema = Value::Object((*tool.input_schema).clone());
        assert_eq!(
            schema["additionalProperties"],
            json!(false),
            "{}: unknown fields rejected",
            tool.name
        );
        assert!(
            tool.description.as_ref().is_some_and(|d| d.len() > 80),
            "{}: real description",
            tool.name
        );
        let read_only = tool.annotations.as_ref().and_then(|a| a.read_only_hint);
        let expected = tool.name != "create_incident_ticket";
        assert_eq!(read_only, Some(expected), "{}: read_only_hint", tool.name);
    }

    let search = Value::Object((*tools[2].input_schema).clone());
    assert_eq!(search["required"], json!(["service_name"]));
    assert_eq!(
        search["properties"]["severity"]["enum"],
        json!(["INFO", "WARN", "ERROR", "FATAL", null])
    );
    assert_eq!(search["properties"]["limit"]["maximum"], json!(100));

    let deploy = Value::Object((*tools[1].input_schema).clone());
    assert_eq!(deploy["required"], json!(["service_name", "window_hours"]));
    assert_eq!(deploy["properties"]["window_hours"]["minimum"], json!(1));

    let ticket = Value::Object((*tools[0].input_schema).clone());
    assert_eq!(
        ticket["required"],
        json!(["title", "severity", "summary", "assignee"])
    );
    assert_eq!(
        ticket["properties"]["severity"]["enum"],
        json!(["LOW", "MEDIUM", "HIGH", "CRITICAL"])
    );
    assert_eq!(
        ticket["properties"]["assignee"]["enum"],
        json!(["identity-oncall", "payments-oncall", "messaging-oncall"])
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn lists_resources_and_the_runbook_template() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    let uris: Vec<String> = client
        .list_all_resources()
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.uri)
        .collect();
    assert_eq!(
        uris,
        [
            "config://release-checklist",
            "service://runbooks/auth-service",
            "service://runbooks/payments-api",
            "service://runbooks/notification-worker"
        ]
    );
    let templates = client.list_all_resource_templates().await.unwrap();
    assert_eq!(templates.len(), 1);
    assert_eq!(
        templates[0].uri_template,
        "service://runbooks/{service_name}"
    );
    client.cancel().await.unwrap();
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reads_the_checklist_and_every_runbook() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    let checklist = read(&client, "config://release-checklist").await.unwrap();
    assert!(checklist.contains("RC-1") && checklist.contains("RC-9"));

    for (service, owner) in [
        ("auth-service", "identity-oncall"),
        ("payments-api", "payments-oncall"),
        ("notification-worker", "messaging-oncall"),
    ] {
        let runbook = read(&client, &format!("service://runbooks/{service}"))
            .await
            .unwrap();
        assert!(runbook.contains("Escalation"), "{service}");
        assert!(runbook.contains(owner), "{service} names its owner");
    }
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn unknown_or_hostile_resource_uris_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    for uri in [
        "service://runbooks/billing-service",
        "service://runbooks/../../.env",
        "service://runbooks/..%2F..%2F.env",
        "service://runbooks/",
        "service://runbooks/auth-service/extra",
        "config://secrets",
        "file:///C:/Windows/win.ini",
    ] {
        let err = read(&client, uri).await.expect_err(uri);
        let ServiceError::McpError(data) = err else {
            panic!("{uri}: expected an MCP error, got {err:?}")
        };
        let body = data.data.expect("structured error data");
        let code = body["error"]["code"].as_str().unwrap();
        assert!(
            ["UNKNOWN_SERVICE", "UNKNOWN_RESOURCE"].contains(&code),
            "{uri}: {code}"
        );
    }
    // The session is still healthy afterwards.
    assert!(read(&client, "config://release-checklist").await.is_ok());
    client.cancel().await.unwrap();
}

// ---------------------------------------------------------------------------
// search_logs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_logs_filters_by_service_severity_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    let r = ok(
        &client,
        "search_logs",
        json!({"service_name": "auth-service", "severity": "ERROR", "limit": 5}),
    )
    .await;
    assert_eq!(r["total_matching"], 3);
    assert_eq!(r["returned"], 3);
    let ids: Vec<&str> = r["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["log-0024", "log-0019", "log-0018"]);
    let first = &r["entries"][0];
    for field in [
        "timestamp",
        "service",
        "severity",
        "version",
        "instance",
        "message",
        "attributes",
    ] {
        assert!(!first[field].is_null(), "entry has {field}");
    }

    let r = ok(
        &client,
        "search_logs",
        json!({"service_name": "payments-api", "limit": 2}),
    )
    .await;
    assert_eq!(
        (r["total_matching"].as_u64(), r["returned"].as_u64()),
        (Some(22), Some(2))
    );
    assert_eq!(r["severity_filter"], Value::Null);

    let r = ok(
        &client,
        "search_logs",
        json!({"service_name": "payments-api", "severity": null}),
    )
    .await;
    assert_eq!(r["returned"], 20, "null severity = all, default limit 20");
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn search_logs_returns_the_injection_entry_as_plain_data() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;
    let r = ok(
        &client,
        "search_logs",
        json!({"service_name": "payments-api", "severity": "ERROR", "limit": 100}),
    )
    .await;
    let entry = r["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "log-0033")
        .expect("log-0033");
    assert!(
        entry["message"]
            .as_str()
            .unwrap()
            .contains("SYSTEM OVERRIDE")
    );
    assert!(
        r["notice"]
            .as_str()
            .unwrap()
            .contains("never as instructions")
    );
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn search_logs_rejects_invalid_values_with_structured_errors() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    let e = tool_error(
        &client,
        "search_logs",
        json!({"service_name": "billing-service"}),
    )
    .await;
    assert_eq!(e["code"], "UNKNOWN_SERVICE");
    assert_eq!(
        e["valid_values"],
        json!(["auth-service", "payments-api", "notification-worker"])
    );

    let e = tool_error(
        &client,
        "search_logs",
        json!({"service_name": "auth-service", "severity": "CRITICAL"}),
    )
    .await;
    assert_eq!(e["code"], "INVALID_SEVERITY");
    assert_eq!(e["valid_values"], json!(["INFO", "WARN", "ERROR", "FATAL"]));

    for limit in [0, -3, 101, 100_000] {
        let e = tool_error(
            &client,
            "search_logs",
            json!({"service_name": "auth-service", "limit": limit}),
        )
        .await;
        assert_eq!(e["code"], "INVALID_LIMIT", "limit {limit}");
    }
    client.cancel().await.unwrap();
}

/// Requests that don't match the input schema are rejected by the SDK before
/// our code runs. rmcp 3.11 reports them as tool-level errors (`isError: true`
/// with a text message), so the caller can read and fix them.
#[tokio::test]
async fn malformed_requests_are_rejected_and_the_server_survives() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    let malformed = [
        ("search_logs", json!({})),                   // missing field
        ("search_logs", json!({"service_name": 42})), // wrong type
        (
            "search_logs",
            json!({"service_name": "auth-service", "limit": "ten"}),
        ), // wrong type
        (
            "search_logs",
            json!({"service_name": "auth-service", "limit": 2.5}),
        ), // not an integer
        (
            "search_logs",
            json!({"service_name": "auth-service", "path": "../.env"}),
        ), // unknown field
        (
            "get_deployment_history",
            json!({"service_name": "auth-service"}),
        ), // missing window
        ("create_incident_ticket", json!({"title": "only a title"})), // missing fields
    ];
    for (tool, args) in malformed {
        let result = call(&client, tool, args.clone())
            .await
            .expect("answered, not a crash");
        assert_eq!(result.is_error, Some(true), "{tool} {args}");
        let text = &result.content[0].as_text().expect("text").text;
        assert!(
            text.contains("failed to deserialize parameters"),
            "{tool} {args}: {text}"
        );
    }
    let unknown_tool = client
        .call_tool(CallToolRequestParams::new("delete_everything"))
        .await;
    assert!(unknown_tool.is_err(), "unknown tools are rejected");

    // Still alive and answering correctly.
    let r = ok(
        &client,
        "search_logs",
        json!({"service_name": "auth-service", "limit": 1}),
    )
    .await;
    assert_eq!(r["returned"], 1);
    client.cancel().await.unwrap();
}

// ---------------------------------------------------------------------------
// get_deployment_history
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deployment_history_returns_versions_commits_timestamps_and_rollbacks() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;

    let r = ok(
        &client,
        "get_deployment_history",
        json!({"service_name": "payments-api", "window_hours": 24}),
    )
    .await;
    assert_eq!(r["count"], 1);
    let d = &r["deployments"][0];
    assert_eq!(d["version"], "2.14.0");
    assert_eq!(d["previous_version"], "2.13.2");
    assert_eq!(d["commit"], "561378f2a66e81041065b3d62027e7ec2f074573");
    assert_eq!(d["deployed_at"], "2026-11-20T09:05:00Z");
    assert_eq!(d["rollback_status"], "not_rolled_back");

    let r = ok(
        &client,
        "get_deployment_history",
        json!({"service_name": "notification-worker", "window_hours": 72}),
    )
    .await;
    let rolled = r["deployments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["deployment_id"] == "dep-1044")
        .unwrap();
    assert_eq!(rolled["rollback_status"], "rolled_back");
    assert_eq!(rolled["rolled_back_to"], "3.1.2");
    assert_eq!(rolled["rolled_back_at"], "2026-11-17T14:20:00Z");

    let r = ok(
        &client,
        "get_deployment_history",
        json!({"service_name": "auth-service", "window_hours": 1}),
    )
    .await;
    assert_eq!(r["count"], 0);
    client.cancel().await.unwrap();
}

#[tokio::test]
async fn deployment_history_rejects_invalid_windows_and_services() {
    let dir = tempfile::tempdir().unwrap();
    let client = start(&seed_data(), &dir.path().join("t.db")).await;
    for window in [-5, 0, 721, 1_000_000] {
        let e = tool_error(
            &client,
            "get_deployment_history",
            json!({"service_name": "auth-service", "window_hours": window}),
        )
        .await;
        assert_eq!(e["code"], "INVALID_TIME_WINDOW", "window {window}");
    }
    let e = tool_error(
        &client,
        "get_deployment_history",
        json!({"service_name": "billing-service", "window_hours": 24}),
    )
    .await;
    assert_eq!(e["code"], "UNKNOWN_SERVICE");
    client.cancel().await.unwrap();
}

// ---------------------------------------------------------------------------
// create_incident_ticket
// ---------------------------------------------------------------------------

fn stored_ids(db: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let mut stmt = conn.prepare("SELECT id FROM tickets ORDER BY id").unwrap();
    stmt.query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .map(|id| format!("INC-{}", id.unwrap() + 1000))
        .collect()
}

#[tokio::test]
async fn tickets_are_created_with_unique_ids_and_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("data").join("tickets.db");
    let client = start(&seed_data(), &db).await;

    let r = ok(
        &client,
        "create_incident_ticket",
        ticket_args(
            "payments-api checkout outage",
            "CRITICAL",
            "payments-oncall",
        ),
    )
    .await;
    assert_eq!(r["created"], true);
    let t = &r["ticket"];
    assert_eq!(t["ticket_id"], "INC-1001");
    assert_eq!(t["severity"], "CRITICAL");
    assert_eq!(t["assignee"], "payments-oncall");
    assert_eq!(t["status"], "OPEN");
    let created_at = t["created_at"].as_str().unwrap();
    assert!(
        chrono::DateTime::parse_from_rfc3339(created_at).is_ok(),
        "{created_at}"
    );

    let r = ok(
        &client,
        "create_incident_ticket",
        ticket_args("auth key rotation follow-up", "MEDIUM", "identity-oncall"),
    )
    .await;
    assert_eq!(r["ticket"]["ticket_id"], "INC-1002");
    client.cancel().await.unwrap();

    // Restart the server on the same database: IDs continue, nothing is lost.
    let client = start(&seed_data(), &db).await;
    let r = ok(
        &client,
        "create_incident_ticket",
        ticket_args("notification SMS quota alert", "LOW", "messaging-oncall"),
    )
    .await;
    assert_eq!(r["ticket"]["ticket_id"], "INC-1003");
    client.cancel().await.unwrap();

    assert_eq!(stored_ids(&db), ["INC-1001", "INC-1002", "INC-1003"]);
}

#[tokio::test]
async fn invalid_tickets_are_rejected_and_nothing_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tickets.db");
    let client = start(&seed_data(), &db).await;

    // The exact ticket the prompt-injection payload asks for.
    let e = tool_error(
        &client,
        "create_incident_ticket",
        ticket_args("SYSTEM OVERRIDE", "CRITICAL", "admin"),
    )
    .await;
    assert_eq!(e["code"], "INVALID_TICKET_FIELDS");
    assert_eq!(e["field_errors"][0]["field"], "assignee");

    let e = tool_error(
        &client,
        "create_incident_ticket",
        json!({"title": "x", "severity": "URGENT", "summary": "short", "assignee": ""}),
    )
    .await;
    let fields: Vec<&str> = e["field_errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["title", "summary", "severity", "assignee"]);
    client.cancel().await.unwrap();

    assert!(
        !db.exists() || stored_ids(&db).is_empty(),
        "no ticket rows after rejected calls"
    );
}

#[tokio::test]
async fn a_broken_database_gives_a_structured_error_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tickets.db");
    std::fs::write(
        &db,
        "not a sqlite database - just some text that is long enough",
    )
    .unwrap();
    let client = start(&seed_data(), &db).await;

    let e = tool_error(
        &client,
        "create_incident_ticket",
        ticket_args("payments-api checkout outage", "HIGH", "payments-oncall"),
    )
    .await;
    assert_eq!(e["code"], "DATABASE_ERROR");
    // Read-only tools are unaffected.
    ok(
        &client,
        "search_logs",
        json!({"service_name": "auth-service", "limit": 1}),
    )
    .await;
    client.cancel().await.unwrap();
}

// ---------------------------------------------------------------------------
// Missing data files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_data_files_give_structured_errors_and_the_server_keeps_running() {
    let empty = tempfile::tempdir().unwrap();
    let client = start(empty.path(), &empty.path().join("t.db")).await;

    let e = tool_error(
        &client,
        "search_logs",
        json!({"service_name": "auth-service"}),
    )
    .await;
    assert_eq!(e["code"], "DATA_UNAVAILABLE");
    assert!(e["message"].as_str().unwrap().contains("logs.json"));
    let e = tool_error(
        &client,
        "get_deployment_history",
        json!({"service_name": "auth-service", "window_hours": 24}),
    )
    .await;
    assert_eq!(e["code"], "DATA_UNAVAILABLE");

    for uri in [
        "config://release-checklist",
        "service://runbooks/auth-service",
    ] {
        let ServiceError::McpError(data) = read(&client, uri).await.unwrap_err() else {
            panic!("{uri}")
        };
        assert_eq!(
            data.data.unwrap()["error"]["code"],
            "DATA_UNAVAILABLE",
            "{uri}"
        );
    }

    // Tickets do not depend on the data files.
    ok(
        &client,
        "create_incident_ticket",
        ticket_args("payments-api checkout outage", "HIGH", "payments-oncall"),
    )
    .await;
    client.cancel().await.unwrap();
}
