//! The tool registry: what was discovered from the MCP servers at runtime,
//! turned into tools Claude can call.
//!
//! For every discovered tool we keep:
//! * the **namespaced name** Claude sees (`<server>__<tool>`, e.g.
//!   `telemetry__search_logs`), which says which server owns it;
//! * the owning server and the original MCP tool name (for routing);
//! * the description;
//! * the **original MCP input schema** (used to validate Claude's arguments)
//!   and an **adapted schema** for the Anthropic API;
//! * a **permission class** (read-only / requires approval / blocked).
//!
//! Nothing here is hard-coded per tool except the short policy list of write
//! tools that need human approval. Anything that is neither annotated
//! read-only nor on that list is **blocked** (fail closed) and never shown to
//! Claude.

use std::collections::BTreeMap;
use std::fmt;

use rmcp::model::{Resource, ResourceTemplate, Tool};
use serde_json::{Map, Value, json};

use crate::llm::ToolDefinition;

/// Separator between server and tool name in what Claude sees.
pub const NAMESPACE_SEP: &str = "__";
/// Write tools that may run only after a human approves the exact call.
pub const APPROVAL_REQUIRED: [(&str, &str); 1] = [("telemetry", "create_incident_ticket")];
const READ_RESOURCE: &str = "read_resource";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Runs automatically.
    ReadOnly,
    /// A call is a *proposal*; it runs only after human approval (Phase 6).
    RequiresApproval,
    /// Never offered to Claude, never executed.
    Blocked,
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ReadOnly => "read-only",
            Self::RequiresApproval => "requires approval",
            Self::Blocked => "blocked",
        })
    }
}

/// How a registered tool is executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolTarget {
    /// `tools/call` of this MCP tool on the owning server.
    McpTool(String),
    /// Generated tool: `resources/read` on the owning server.
    ReadResource {
        uris: Vec<String>,
        templates: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct RegisteredTool {
    pub name: String,
    pub server: &'static str,
    pub target: ToolTarget,
    pub description: String,
    /// The schema exactly as the MCP server published it.
    pub mcp_schema: Value,
    /// The schema sent to Claude.
    pub claude_schema: Value,
    pub permission: Permission,
}

/// What one server offered during discovery.
#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub name: &'static str,
    pub label: String,
    pub tools: Vec<Tool>,
    pub resources: Vec<Resource>,
    pub templates: Vec<ResourceTemplate>,
}

#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
    /// (namespaced name, reason) of tools that were not registered.
    pub excluded: Vec<(String, String)>,
}

impl ToolRegistry {
    pub fn build(servers: &[DiscoveredServer]) -> Self {
        let mut registry = Self::default();
        for server in servers {
            for tool in &server.tools {
                registry.add_mcp_tool(server.name, tool);
            }
            if !server.resources.is_empty() {
                registry.add_resource_tool(server);
            }
        }
        registry
    }

    fn add_mcp_tool(&mut self, server: &'static str, tool: &Tool) {
        let name = format!("{server}{NAMESPACE_SEP}{}", tool.name);
        let permission = classify(server, tool);
        if permission == Permission::Blocked {
            self.excluded.push((
                name,
                "not annotated read-only and not an approved write tool".into(),
            ));
            return;
        }
        if let Err(reason) = valid_tool_name(&name) {
            self.excluded.push((name, reason));
            return;
        }
        let mcp_schema = Value::Object((*tool.input_schema).clone());
        let claude_schema = match adapt_schema(&mcp_schema) {
            Ok(s) => s,
            Err(reason) => {
                self.excluded.push((name, reason));
                return;
            }
        };
        let original = tool
            .description
            .as_deref()
            .unwrap_or("(no description)")
            .trim();
        let description = match permission {
            Permission::RequiresApproval => format!(
                "[{server} server | WRITE - requires human approval] {original} Calling this tool only \
                 PROPOSES the action: a human must approve the exact arguments before anything is executed."
            ),
            _ => format!("[{server} server | read-only] {original}"),
        };
        self.tools.insert(
            name.clone(),
            RegisteredTool {
                name,
                server,
                target: ToolTarget::McpTool(tool.name.to_string()),
                description,
                mcp_schema,
                claude_schema,
                permission,
            },
        );
    }

    /// One generated `<server>__read_resource` tool for a server that lists
    /// resources. Its description and allowed URIs come from discovery.
    fn add_resource_tool(&mut self, server: &DiscoveredServer) {
        let name = format!("{}{NAMESPACE_SEP}{READ_RESOURCE}", server.name);
        if self.tools.contains_key(&name) {
            self.excluded
                .push((name, "name clash with a real tool".into()));
            return;
        }
        let uris: Vec<String> = server.resources.iter().map(|r| r.uri.clone()).collect();
        let templates: Vec<String> = server
            .templates
            .iter()
            .map(|t| t.uri_template.clone())
            .collect();
        let mut lines = vec![format!(
            "[{} server | read-only] Read one MCP resource (a document) by its URI. Available resources:",
            server.name
        )];
        for r in &server.resources {
            let what = r
                .description
                .as_deref()
                .or(r.title.as_deref())
                .unwrap_or(&r.name);
            lines.push(format!("- {}: {what}", r.uri));
        }
        for t in &server.templates {
            lines.push(format!(
                "- template {}: {}",
                t.uri_template,
                t.description.as_deref().unwrap_or(&t.name)
            ));
        }
        let schema = json!({
            "type": "object",
            "properties": {
                "uri": {"type": "string", "description": "Exact resource URI from the list in the tool description."}
            },
            "required": ["uri"],
            "additionalProperties": false
        });
        self.tools.insert(
            name.clone(),
            RegisteredTool {
                name,
                server: server.name,
                target: ToolTarget::ReadResource { uris, templates },
                description: lines.join("\n"),
                mcp_schema: schema.clone(),
                claude_schema: schema,
                permission: Permission::ReadOnly,
            },
        );
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &RegisteredTool> {
        self.tools.values()
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Tool definitions for the Messages API, sorted by name (a stable order
    /// keeps the prompt-cache prefix identical between requests).
    pub fn claude_tools(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.claude_schema.clone(),
            })
            .collect()
    }
}

fn classify(server: &str, tool: &Tool) -> Permission {
    if APPROVAL_REQUIRED
        .iter()
        .any(|(s, t)| *s == server && *t == tool.name)
    {
        Permission::RequiresApproval
    } else if tool.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true) {
        Permission::ReadOnly
    } else {
        Permission::Blocked
    }
}

/// Claude tool names: letters, digits, `_` and `-`, at most 64 characters.
fn valid_tool_name(name: &str) -> Result<(), String> {
    if !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Ok(())
    } else {
        Err("name is not a valid Claude tool name (letters, digits, _ and -, max 64)".into())
    }
}

/// Adapts an MCP input schema for the Anthropic API.
///
/// MCP servers publish full JSON Schema documents; Claude's `input_schema` must
/// be an object schema. We: require `type: "object"` (adding it when absent),
/// make sure `properties` exists, and drop document-level keywords
/// (`$schema`, `$id`) that describe the document rather than the input.
/// Everything else (descriptions, enums, ranges, `additionalProperties`) is
/// kept so Claude sees the constraints. The original stays in the registry
/// for our own validation.
pub fn adapt_schema(schema: &Value) -> Result<Value, String> {
    let Some(obj) = schema.as_object() else {
        return Err("input schema is not a JSON object".into());
    };
    let mut out: Map<String, Value> = obj.clone();
    out.remove("$schema");
    out.remove("$id");
    match out.get("type") {
        None => {
            out.insert("type".into(), json!("object"));
        }
        Some(Value::String(t)) if t == "object" => {}
        Some(other) => return Err(format!("input schema type is {other}, not \"object\"")),
    }
    if !out.get("properties").is_some_and(Value::is_object) {
        out.insert("properties".into(), json!({}));
    }
    let adapted = Value::Object(out);
    if adapted.to_string().len() > 64 * 1024 {
        return Err("input schema is larger than 64 KiB".into());
    }
    Ok(adapted)
}

/// Checks a URI against the resources and templates a server advertised.
/// A template variable matches one non-empty path segment (no `/`, no `..`).
pub fn resource_uri_allowed(uri: &str, uris: &[String], templates: &[String]) -> bool {
    if uris.iter().any(|u| u == uri) {
        return true;
    }
    templates.iter().any(|t| {
        let (Some(open), Some(close)) = (t.find('{'), t.find('}')) else {
            return false;
        };
        let (prefix, suffix) = (&t[..open], &t[close + 1..]);
        uri.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .is_some_and(|value| {
                !value.is_empty()
                    && !value.contains(['/', '\\', '?', '#', '%'])
                    && value != ".."
                    && value != "."
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ToolAnnotations;
    use std::sync::Arc;

    fn tool(name: &str, read_only: Option<bool>, schema: Value) -> Tool {
        let mut t = Tool::new(
            name.to_string(),
            format!("{name} description"),
            Arc::new(schema.as_object().unwrap().clone()),
        );
        t.annotations = read_only.map(|r| ToolAnnotations::new().read_only(r));
        t
    }

    fn object_schema() -> Value {
        json!({"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "object",
               "properties": {"service_name": {"type": "string"}}, "required": ["service_name"]})
    }

    fn servers() -> Vec<DiscoveredServer> {
        vec![
            DiscoveredServer {
                name: "telemetry",
                label: "ops-telemetry-mcp".into(),
                tools: vec![
                    tool("search_logs", Some(true), object_schema()),
                    tool("create_incident_ticket", Some(false), object_schema()),
                    tool("drop_database", Some(false), object_schema()),
                    tool("mystery", None, object_schema()),
                ],
                resources: vec![Resource::new(
                    "config://release-checklist",
                    "release-checklist",
                )],
                templates: vec![ResourceTemplate::new(
                    "service://runbooks/{service_name}",
                    "runbook",
                )],
            },
            DiscoveredServer {
                name: "files",
                label: "fs".into(),
                tools: vec![
                    tool(
                        "read_text_file",
                        Some(true),
                        json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                    ),
                    tool("write_file", Some(false), object_schema()),
                ],
                resources: vec![],
                templates: vec![ResourceTemplate::new("file://{+path}", "file")],
            },
        ]
    }

    #[test]
    fn tools_are_namespaced_classified_and_mapped_to_their_server() {
        let reg = ToolRegistry::build(&servers());
        let names: Vec<&str> = reg.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "files__read_text_file",
                "telemetry__create_incident_ticket",
                "telemetry__read_resource",
                "telemetry__search_logs"
            ]
        );
        assert_eq!(reg.get("files__read_text_file").unwrap().server, "files");
        assert_eq!(
            reg.get("telemetry__search_logs").unwrap().permission,
            Permission::ReadOnly
        );
        let ticket = reg.get("telemetry__create_incident_ticket").unwrap();
        assert_eq!(ticket.permission, Permission::RequiresApproval);
        assert!(ticket.description.contains("requires human approval"));
        assert_eq!(
            ticket.target,
            ToolTarget::McpTool("create_incident_ticket".into())
        );

        let excluded: Vec<&str> = reg.excluded.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            excluded,
            [
                "telemetry__drop_database",
                "telemetry__mystery",
                "files__write_file"
            ]
        );
    }

    #[test]
    fn the_resource_tool_lists_discovered_resources() {
        let reg = ToolRegistry::build(&servers());
        let t = reg.get("telemetry__read_resource").unwrap();
        assert!(t.description.contains("config://release-checklist"));
        assert!(t.description.contains("service://runbooks/{service_name}"));
        assert!(
            reg.get("files__read_resource").is_none(),
            "files lists no resources"
        );
    }

    #[test]
    fn claude_tools_are_sorted_and_adapted() {
        let tools = ToolRegistry::build(&servers()).claude_tools();
        let mut sorted = tools.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(tools, sorted);
        for t in &tools {
            assert_eq!(t.input_schema["type"], "object", "{}", t.name);
            assert!(t.input_schema.get("$schema").is_none(), "{}", t.name);
            assert!(t.input_schema["properties"].is_object(), "{}", t.name);
        }
    }

    #[test]
    fn schema_adaptation() {
        let adapted =
            adapt_schema(&json!({"properties": {"a": {"type": "integer", "minimum": 1}}})).unwrap();
        assert_eq!(adapted["type"], "object");
        assert_eq!(
            adapted["properties"]["a"]["minimum"], 1,
            "constraints are kept"
        );
        assert!(adapt_schema(&json!({"type": "string"})).is_err());
        assert!(adapt_schema(&json!([1, 2])).is_err());
        assert_eq!(
            adapt_schema(&json!({"type": "object"})).unwrap()["properties"],
            json!({})
        );
    }

    #[test]
    fn resource_uris_are_checked_against_discovery() {
        let uris = vec!["config://release-checklist".to_string()];
        let templates = vec!["service://runbooks/{service_name}".to_string()];
        let ok = |u: &str| resource_uri_allowed(u, &uris, &templates);
        assert!(ok("config://release-checklist"));
        assert!(ok("service://runbooks/payments-api"));
        assert!(
            ok("service://runbooks/billing-service"),
            "the server decides if it exists"
        );
        for bad in [
            "service://runbooks/",
            "service://runbooks/../../.env",
            "service://runbooks/a/b",
            "service://runbooks/..%2F.env",
            "config://secrets",
            "file:///C:/Windows/win.ini",
        ] {
            assert!(!ok(bad), "{bad}");
        }
    }

    #[test]
    fn invalid_names_are_excluded() {
        assert!(valid_tool_name("telemetry__search_logs").is_ok());
        assert!(valid_tool_name("bad name").is_err());
        assert!(valid_tool_name(&"x".repeat(65)).is_err());
    }
}
