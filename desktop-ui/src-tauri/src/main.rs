// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub sender: String,
    pub content: String,
    pub timestamp: String,
    pub tool_calls: Option<Vec<ToolCallInfo>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallInfo {
    pub server: String,
    pub tool: String,
    pub status: String,
    pub args: serde_json::Value,
    pub result: Option<serde_json::Value>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
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
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err("Prompt cannot be empty".to_string());
    }

    tokio::task::spawn_blocking(move || run_agent_process(&prompt))
        .await
        .map_err(|e| format!("Agent task failed: {e}"))?
}

fn run_agent_process(prompt: &str) -> Result<ChatMessage, String> {
    let root = repo_root();
    let executable = root.join("target").join("debug").join("agent-client.exe");
    if !executable.is_file() {
        return Err(format!(
            "Agent executable not found at {}. Run `cargo build --workspace` first.",
            executable.display()
        ));
    }

    let output = Command::new(executable)
        .arg("ask")
        .arg(prompt)
        .current_dir(&root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Could not start the agent: {e}"))?;

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        combined.push('\n');
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
    }

    let content = extract_answer(&combined).unwrap_or_else(|| {
        if combined.trim().is_empty() {
            "The agent returned no output.".to_string()
        } else {
            combined.trim().to_string()
        }
    });

    if !output.status.success() && extract_answer(&combined).is_none() {
        return Err(content);
    }

    Ok(ChatMessage {
        id: format!("msg-{}", chrono::Utc::now().timestamp_millis()),
        sender: "agent".into(),
        content,
        timestamp: chrono::Utc::now().to_rfc3339(),
        tool_calls: Some(parse_tool_calls(&combined)),
    })
}

fn extract_answer(output: &str) -> Option<String> {
    let answer_start = output.rfind("claude> ")?;
    let answer = &output[answer_start + "claude> ".len()..];
    let answer = answer
        .split("\n\n  [")
        .next()
        .unwrap_or(answer)
        .trim();
    (!answer.is_empty()).then(|| answer.to_string())
}

fn parse_tool_calls(output: &str) -> Vec<ToolCallInfo> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let rest = line.strip_prefix("-> ")?;
            let mut parts = rest.splitn(2, char::is_whitespace);
            let name = parts.next()?.trim();
            let args_text = parts.next().unwrap_or_default().trim();
            let args = serde_json::from_str::<serde_json::Value>(args_text)
                .unwrap_or_else(|_| serde_json::json!({ "raw": args_text }));
            let (server, tool) = name
                .split_once("__")
                .map(|(server, tool)| (server, tool))
                .unwrap_or(("agent", name));
            Some(ToolCallInfo {
                server: server.to_string(),
                tool: tool.to_string(),
                status: "success".to_string(),
                args,
                result: None,
            })
        })
        .collect()
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
