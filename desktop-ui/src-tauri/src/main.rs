// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
    pub transport: String,
    pub tools_count: usize,
    pub resources_count: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TelemetryLog {
    pub id: String,
    pub timestamp: String,
    pub service: String,
    pub severity: String,
    pub instance_id: String,
    pub message: String,
    pub attributes: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DeploymentRecord {
    pub deployment_id: String,
    pub service: String,
    pub version: String,
    pub environment: String,
    pub deployed_at: String,
    pub deployed_by: String,
    pub status: String,
    pub rolled_back_at: Option<String>,
    pub commit_hash: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ChatMessage {
    pub id: String,
    pub sender: String,
    pub content: String,
    pub timestamp: String,
    pub tool_calls: Option<Vec<ToolCallInfo>>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ToolCallInfo {
    pub server: String,
    pub tool: String,
    pub status: String,
    pub args: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

fn repo_root() -> PathBuf {
    // Current dir or parent of src-tauri
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[tauri::command]
fn get_mcp_status() -> Vec<McpServerStatus> {
    vec![
        McpServerStatus {
            name: "ops-telemetry-mcp".into(),
            status: "connected".into(),
            transport: "stdio (JSON-RPC 2.0)".into(),
            tools_count: 4,
            resources_count: 4,
        },
        McpServerStatus {
            name: "filesystem-mcp".into(),
            status: "connected".into(),
            transport: "stdio (JSON-RPC 2.0)".into(),
            tools_count: 10,
            resources_count: 0,
        },
    ]
}

#[tauri::command]
fn search_logs(service: Option<String>, severity: Option<String>) -> Result<Vec<TelemetryLog>, String> {
    let mut candidate_paths = vec![
        repo_root().join("mock_data").join("logs.json"),
        repo_root().join("..").join("mock_data").join("logs.json"),
        PathBuf::from("mock_data/logs.json"),
    ];

    let mut content = None;
    for p in candidate_paths.drain(..) {
        if let Ok(c) = fs::read_to_string(&p) {
            content = Some(c);
            break;
        }
    }

    let json_str = content.ok_or_else(|| "Could not find mock_data/logs.json".to_string())?;
    let all_logs: Vec<TelemetryLog> = serde_json::from_str(&json_str).map_err(|e| e.to_string())?;

    let filtered = all_logs
        .into_iter()
        .filter(|l| {
            if let Some(ref s) = service {
                if !s.is_empty() && s != "all" && l.service != *s {
                    return false;
                }
            }
            if let Some(ref sev) = severity {
                if !sev.is_empty() && sev != "all" && l.severity != *sev {
                    return false;
                }
            }
            true
        })
        .collect();

    Ok(filtered)
}

#[tauri::command]
fn get_deployments(service: Option<String>) -> Result<Vec<DeploymentRecord>, String> {
    let mut candidate_paths = vec![
        repo_root().join("mock_data").join("deployments.json"),
        repo_root().join("..").join("mock_data").join("deployments.json"),
        PathBuf::from("mock_data/deployments.json"),
    ];

    let mut content = None;
    for p in candidate_paths.drain(..) {
        if let Ok(c) = fs::read_to_string(&p) {
            content = Some(c);
            break;
        }
    }

    let json_str = content.ok_or_else(|| "Could not find mock_data/deployments.json".to_string())?;
    let all_deps: Vec<DeploymentRecord> = serde_json::from_str(&json_str).map_err(|e| e.to_string())?;

    let filtered = all_deps
        .into_iter()
        .filter(|d| {
            if let Some(ref s) = service {
                if !s.is_empty() && s != "all" && d.service != *s {
                    return false;
                }
            }
            true
        })
        .collect();

    Ok(filtered)
}

#[tauri::command]
async fn run_agent_turn(prompt: String) -> Result<ChatMessage, String> {
    // Return structured AI reasoning and tool calls
    Ok(ChatMessage {
        id: format!("msg-{}", chrono::Utc::now().timestamp_millis()),
        sender: "agent".into(),
        content: format!(
            "Investigation initiated for: \"{}\".\n\n\
            1. Correlating deployment timeline with telemetry error spikes.\n\
            2. Identified deployment 'dep-1048' (payments-api v2.14.0) deployed at 2026-11-20T09:05:00Z.\n\
            3. Error logs show PayFlow downstream timeout decreased from 10000ms to 2000ms.\n\
            4. Connection pool saturation followed by circuit breaker trip.",
            prompt
        ),
        timestamp: chrono::Utc::now().to_rfc3339(),
        tool_calls: Some(vec![
            ToolCallInfo {
                server: "ops-telemetry-mcp".into(),
                tool: "search_logs".into(),
                status: "success".into(),
                args: serde_json::json!({ "service_name": "payments-api", "severity": "ERROR", "limit": 20 }),
                result: Some(serde_json::json!({ "matches": 6 })),
            },
            ToolCallInfo {
                server: "ops-telemetry-mcp".into(),
                tool: "get_deployment_history".into(),
                status: "success".into(),
                args: serde_json::json!({ "service_name": "payments-api", "window_hours": 24 }),
                result: Some(serde_json::json!({ "active_version": "2.14.0" })),
            },
        ]),
    })
}

#[tauri::command]
fn respond_to_approval(request_id: String, approved: bool, reason: Option<String>) -> bool {
    println!(
        "[HITL Approval] ID: {}, Decision: {}, Reason: {:?}",
        request_id,
        if approved { "APPROVED" } else { "REJECTED" },
        reason
    );
    approved
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_mcp_status,
            search_logs,
            get_deployments,
            run_agent_turn,
            respond_to_approval
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
