# Learning Notes — Phase 6: Guardrails and Security Engineering

## 1. The mindset: treat the model as untrusted
An LLM reads text from the outside world (logs, files) and that text can try to steer it. So the question is not "will Claude resist?" but **"what is the worst a fooled Claude can do?"** Every guardrail in this phase is enforced by *our code or the servers*, never by asking the model nicely. The tests prove it by scripting a *compromised* model that obeys the injected log entry, and then checking that nothing bad happened.

Defence in depth means several independent layers, so one failure isn't fatal:

```
Claude asks → [policy: is this tool allowed at all?]
            → [schema validation: are the arguments well-formed?]
            → [approval gate: did a human approve THIS exact payload?]   (writes only)
            → [hash check: is what we run what was approved?]
            → MCP server → [server-side validation, filesystem confinement]
            → output → [redaction] → [injection scan] → [untrusted label] → Claude
            → everything → [trace file, redacted]
```

## 2. Trusted policy vs. self-description
A tool's description and annotations come **from the server**. A malicious or buggy server could call `delete_everything` "read-only". So the permission comes from an allow-list **in our code** (`READ_ONLY_POLICY`, `APPROVAL_REQUIRED`). Annotations may only make things *stricter*: if our policy says read-only but the server says "writes", we block the tool as a conflict. Unknown tools are blocked: **fail closed**.

## 3. Binding approval to the exact payload
"The human approved a ticket" is not enough: *which* ticket? We hash the payload:

```
SHA-256( server \n tool \n canonical JSON arguments )
```
`serde_json` keeps object keys sorted, so the same arguments always give the same hash ("canonical"). The reviewer sees the hash; the decision carries it; just before execution the agent hashes what it is about to send and compares. If anything changed (an edit that wasn't re-approved, a bug, tampering), the call is blocked. An edit produces a new payload, therefore a new hash, therefore needs its own approval. A test simulates the attack with a `Tamper` step.

Two more details: approval input comes only from the **console** (never from model text or tool output), and **end of input means reject**, so a script or a closed terminal can never approve by accident.

## 4. Prompt injection: detection vs. prevention
* **Prevention** is structural: the model *cannot* read `.env` (no tool reaches it), cannot write without a human, cannot call unlisted tools, cannot change the system prompt.
* **Detection** (`safety/injection.rs`) is a phrase scanner. It is easy to evade with new wording, so it's only a *visibility* layer: Claude gets a SECURITY WARNING above the data, the console shows it, the trace records `injection_flags`.
* **Labelling:** every server output is wrapped in `<tool_output ... trust="untrusted-data">…</tool_output>`. Data containing `</tool_output>` is neutralised, so it can't pretend the data has ended (the same idea as escaping in SQL or HTML).

## 5. Redaction
`safety/redact.rs` removes secrets before anything leaves a trust boundary (to Claude, to the trace, to the console):
* the API key's **exact value**;
* **patterns**: `sk-ant-…`, `ghp_…`, AWS `AKIA…`, PEM private keys, `Bearer …`, `password=…`;
* **sensitive JSON keys** (`token`, `api_key`, …);
* **local paths** → `<project>` / `<home>`, so error messages don't reveal the machine's user name.

Vault references like `${vault:secret/...}` are pointers, not secrets, and are kept. A test checks the real mock data triggers zero redactions (no false positives).

## 6. Timeouts and limits
* `MAX_ITERATIONS` (default 8) caps model calls per request; the loop then stops with a visible note and a valid history.
* Per-tool timeouts (`tokio::time::timeout`) with per-tool overrides. To test a hang we needed a server that hangs, so `tests/fixtures/misbehaving_mcp_server.js` is a 50-line fake MCP server whose `search_logs` never answers and whose `get_deployment_history` kills the process. Writing a deliberately broken component is the honest way to test failure handling.

## 7. Traces
JSONL: one JSON object per line, appended. It's easy to `grep`, stream and load into tools, and a crash can't corrupt earlier lines. Each tool call records who / what / input / duration / status / error / approval / injection flags. The trace is evidence for the report and for audits, so it's redacted like everything else.

## 8. Two lessons from this phase
* **Tests can pollute real data.** The end-to-end tests ran in the project folder and appended to the real `traces/agent_traces.jsonl`. Fixed by giving tests their own `TRACE_FILE`, and the stray lines were checked and removed.
* **Error messages are output too.** "Access denied - C:\Users\<name>\..." told Claude (and a committed sample trace) the Windows user name. Small, but exactly the kind of leak that redaction should cover.
