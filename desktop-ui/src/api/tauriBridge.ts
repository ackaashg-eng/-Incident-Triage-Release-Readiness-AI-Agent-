import { invoke } from '@tauri-apps/api/core';

export interface McpServerStatus {
  name: string;
  status: 'connected' | 'disconnected' | 'error';
  transport: string;
  toolsCount: number;
  resourcesCount: number;
}

export interface TelemetryLog {
  id: string;
  timestamp: string;
  service: string;
  severity: 'INFO' | 'WARN' | 'ERROR' | 'FATAL';
  instance_id: string;
  message: string;
  attributes: Record<string, unknown>;
}

export interface DeploymentRecord {
  deployment_id: string;
  service: string;
  version: string;
  environment: string;
  deployed_at: string;
  deployed_by: string;
  status: 'ACTIVE' | 'ROLLED_BACK' | 'FAILED' | 'PENDING';
  rolled_back_at?: string;
  commit_hash: string;
}

export interface ApprovalRequest {
  id: string;
  server: string;
  tool: string;
  payload: Record<string, unknown>;
  payloadHash: string;
  createdAt: string;
}

export interface ChatMessage {
  id: string;
  sender: 'user' | 'agent' | 'system';
  content: string;
  timestamp: string;
  toolCalls?: Array<{
    server: string;
    tool: string;
    status: 'pending' | 'success' | 'failed' | 'requires_approval';
    args: Record<string, unknown>;
    result?: unknown;
  }>;
}

// Fallback to mock API if not running in Tauri window
const isTauri = () => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export async function checkMcpStatus(): Promise<McpServerStatus[]> {
  if (isTauri()) {
    return await invoke<McpServerStatus[]>('get_mcp_status');
  }
  return [
    { name: 'ops-telemetry-mcp', status: 'connected', transport: 'stdio (JSON-RPC 2.0)', toolsCount: 4, resourcesCount: 4 },
    { name: 'filesystem-mcp', status: 'connected', transport: 'stdio (JSON-RPC 2.0)', toolsCount: 10, resourcesCount: 0 }
  ];
}

export async function fetchLogs(service?: string, severity?: string): Promise<TelemetryLog[]> {
  if (isTauri()) {
    return await invoke<TelemetryLog[]>('search_logs', { service, severity });
  }
  // Mock data fallback for browser preview
  const res = await fetch('/mock/logs.json').catch(() => null);
  if (res && res.ok) {
    const data = await res.json();
    return data;
  }
  return [];
}

export async function fetchDeployments(service?: string): Promise<DeploymentRecord[]> {
  if (isTauri()) {
    return await invoke<DeploymentRecord[]>('get_deployments', { service });
  }
  return [];
}

export async function sendAgentPrompt(prompt: string): Promise<ChatMessage> {
  if (isTauri()) {
    return await invoke<ChatMessage>('run_agent_turn', { prompt });
  }
  // Demo simulation when running in pure web mode
  return {
    id: `msg-${Date.now()}`,
    sender: 'agent',
    content: `Investigated "${prompt}". Checking telemetry via ops-telemetry-mcp and verifying release checklist.`,
    timestamp: new Date().toISOString(),
    toolCalls: [
      {
        server: 'telemetry',
        tool: 'search_logs',
        status: 'success',
        args: { service_name: 'payments-api', severity: 'ERROR' },
        result: { matches: 4 }
      }
    ]
  };
}

export async function submitApproval(requestId: string, approved: boolean, reason?: string): Promise<boolean> {
  if (isTauri()) {
    return await invoke<boolean>('respond_to_approval', { requestId, approved, reason });
  }
  return approved;
}
