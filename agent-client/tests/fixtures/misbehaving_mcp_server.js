// A deliberately misbehaving MCP server (stdio, newline-delimited JSON-RPC),
// used ONLY by agent-client/tests/security.rs to prove the agent survives:
//   search_logs            -> never answers   (tool timeout test)
//   get_deployment_history -> kills the server (MCP disconnection test)
// It announces itself as the "telemetry" server's tools so the normal
// application policy classifies them as read-only.
const readline = require("readline");

const rl = readline.createInterface({ input: process.stdin });
const send = (msg) => process.stdout.write(JSON.stringify(msg) + "\n");
const reply = (id, result) => send({ jsonrpc: "2.0", id, result });
const fail = (id, code, message) => send({ jsonrpc: "2.0", id, error: { code, message } });

const tool = (name, description) => ({
  name,
  description,
  inputSchema: { type: "object", properties: {} },
  annotations: { readOnlyHint: true },
});

rl.on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.id === undefined || msg.id === null) return; // notification
  switch (msg.method) {
    case "initialize":
      return reply(msg.id, {
        protocolVersion: "2025-11-25",
        capabilities: { tools: {} },
        serverInfo: { name: "misbehaving-test-server", version: "0.0.1" },
      });
    case "ping":
      return reply(msg.id, {});
    case "tools/list":
      return reply(msg.id, {
        tools: [
          tool("search_logs", "Never answers (timeout test)."),
          tool("get_deployment_history", "Exits the process (disconnect test)."),
        ],
      });
    case "tools/call":
      if (msg.params && msg.params.name === "search_logs") return; // hang forever
      if (msg.params && msg.params.name === "get_deployment_history") process.exit(3);
      return fail(msg.id, -32602, "unknown tool");
    default:
      return fail(msg.id, -32601, "Method not found");
  }
});
