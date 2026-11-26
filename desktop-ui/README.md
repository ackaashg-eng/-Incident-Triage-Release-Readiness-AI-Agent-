# Desktop UI (Tauri v2 + React + TypeScript + Tailwind CSS)

This directory contains the desktop GUI client for the **Incident Triage & Release Readiness AI Agent** built with **Tauri v2** and **React 19**.

---

## 1. Capabilities

- 🤖 **Autonomous Incident Investigation**: Interactive chat console allowing users to query the incident agent, view step-by-step reasoning, and inspect tool calls into `ops-telemetry-mcp` and `files`.
- 🛡️ **Human-in-the-Loop (HITL) Gate**: Visual review panel for proposed state-changing actions (e.g. `create_incident_ticket`), displaying full arguments, assignee, and the cryptographically bound **SHA-256 payload hash** before execution.
- 📋 **Telemetry Log Explorer**: Interactive log viewer backed by [`mock_data/logs.json`](../mock_data/logs.json) with filtering by service (`payments-api`, `auth-service`, `notification-worker`) and severity (`INFO`, `WARN`, `ERROR`, `FATAL`).
- ⏱️ **Deployment History & Rollback Status**: Chronological deployment timeline view from [`mock_data/deployments.json`](../mock_data/deployments.json).
- 🔌 **MCP Server Infrastructure Monitor**: Live connection status, transports, and registered tool counts for connected MCP servers.

---

## 2. Running in Development Mode

### Web Browser Preview (Vite dev server)
For rapid frontend testing without launching the full Tauri shell:
```powershell
cd desktop-ui
npm run dev
```
Open [http://localhost:1420](http://localhost:1420) in your browser.

### Full Tauri Desktop App
To run the native Windows desktop app using the Tauri v2 engine:
```powershell
cd desktop-ui
npx tauri dev
```

---

## 3. Building for Production

To create a standalone, optimized Windows executable (`.exe` / `.msi`):
```powershell
cd desktop-ui
npx tauri build
```
The compiled binary will be placed under:
`desktop-ui/src-tauri/target/release/`
