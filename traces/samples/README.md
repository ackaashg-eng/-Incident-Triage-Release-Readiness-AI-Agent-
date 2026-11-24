# Sample execution traces

Real records written by the agent's tracer (`agent-client/src/trace.rs`) during actual executions. One JSON object per line; format in [docs/SECURITY.md §5](../../docs/SECURITY.md).

**Provenance, stated plainly:** these runs used the real `agent-client` binary, the real MCP servers (ops-telemetry-mcp and the Node filesystem server), the real policy, validation, approval gate, redaction and tracer. The **model's replies** came from a local scripted endpoint (`api_endpoint: http://127.0.0.1:…` in each `session_start` line) because the Anthropic account had no API credits during development. Every tool result, error, timing, approval decision and hash in these files was produced by the real system. A live-Claude trace should be added when credits are available (`.\scripts\run.ps1 ask "…"`, then copy the session from `traces\agent_traces.jsonl`).

| File | What it shows | How it was produced |
|---|---|---|
| `investigation-with-hitl-approval.jsonl` (+ `.console.txt`) | A complete multi-step investigation: logs (injection flagged) + deployments → runbook + RELEASE_NOTES + service-config → ticket proposal → **console approval** (`a`, SHA-256 recorded) → INC-1001 created → assessment | `cargo test -p agent-client --test cli_e2e -- --ignored --test-threads=1` (`write_submission_traces`): `agent-client ask "…"` with `a` typed at the approval prompt |
| `handled-failures-and-edge-cases.jsonl` (+ `.console.txt`) | Unknown service (`UNKNOWN_SERVICE`), injected log flagged, `.env` read denied, unauthorized `files__write_file` refused, `admin` ticket stopped before approval, invalid `window_hours`, then the Claude API overloaded (529) on every retry → request rolled back, clean exit | same test |
| `phase6-scripted-security-demo.jsonl` | Guardrails in one run (library-level, Phase 6): injection flagged, `.env` denied, invalid ticket not reached, valid ticket approved | `cargo test -p agent-client --test security -- --ignored` |

No secrets and no local paths: the API key was a test value and is redacted anyway; local folders appear as `<project>` / `<home>` (checked by `scripts\audit.ps1`).
