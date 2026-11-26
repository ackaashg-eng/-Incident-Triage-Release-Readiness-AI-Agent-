import React, { useState, useEffect, useRef } from 'react';
import { 
  Send, ShieldAlert, Bot, User, Terminal, CheckCircle2, 
  XCircle, Clock, FileText, Server, Layers, AlertTriangle, RefreshCw
} from 'lucide-react';
import { 
  checkMcpStatus, fetchLogs, fetchDeployments, sendAgentPrompt, submitApproval,
  McpServerStatus, TelemetryLog, DeploymentRecord, ApprovalRequest, ChatMessage 
} from './api/tauriBridge';

export default function App() {
  const [activeTab, setActiveTab] = useState<'chat' | 'telemetry' | 'deployments' | 'mcp' | 'approvals'>('chat');
  const [messages, setMessages] = useState<ChatMessage[]>([
    {
      id: 'welcome',
      sender: 'system',
      content: 'Production Incident Triage & Release Readiness AI Agent initialized. Connected to ops-telemetry-mcp and files over stdio JSON-RPC.',
      timestamp: new Date().toISOString()
    }
  ]);
  const [inputPrompt, setInputPrompt] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [servers, setServers] = useState<McpServerStatus[]>([]);
  const [logs, setLogs] = useState<TelemetryLog[]>([]);
  const [deployments, setDeployments] = useState<DeploymentRecord[]>([]);
  const [logFilterService, setLogFilterService] = useState<string>('all');
  const [logFilterSeverity, setLogFilterSeverity] = useState<string>('all');
  const [pendingApprovals, setPendingApprovals] = useState<ApprovalRequest[]>([
    {
      id: 'appr-demo-1',
      server: 'ops-telemetry-mcp',
      tool: 'telemetry__create_incident_ticket',
      payload: {
        title: 'Payments API downstream timeout outage',
        severity: 'CRITICAL',
        summary: 'Circuit breaker tripped due to 2000ms PayFlow timeout in v2.14.0 release.',
        assignee: 'payments-oncall'
      },
      payloadHash: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
      createdAt: new Date().toISOString()
    }
  ]);

  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    loadMcpStatus();
    loadLogs();
    loadDeployments();
  }, []);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [messages]);

  const loadMcpStatus = async () => {
    try {
      const data = await checkMcpStatus();
      setServers(data);
    } catch (e) {
      console.error(e);
    }
  };

  const loadLogs = async () => {
    try {
      const s = logFilterService === 'all' ? undefined : logFilterService;
      const sev = logFilterSeverity === 'all' ? undefined : logFilterSeverity;
      const data = await fetchLogs(s, sev);
      setLogs(data);
    } catch (e) {
      console.error(e);
    }
  };

  const loadDeployments = async () => {
    try {
      const data = await fetchDeployments();
      setDeployments(data);
    } catch (e) {
      console.error(e);
    }
  };

  const handleSend = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!inputPrompt.trim() || isLoading) return;

    const userMsg: ChatMessage = {
      id: `usr-${Date.now()}`,
      sender: 'user',
      content: inputPrompt,
      timestamp: new Date().toISOString()
    };

    setMessages((prev) => [...prev, userMsg]);
    const promptToSend = inputPrompt;
    setInputPrompt('');
    setIsLoading(true);

    try {
      const response = await sendAgentPrompt(promptToSend);
      setMessages((prev) => [...prev, response]);
    } catch (err) {
      setMessages((prev) => [
        ...prev,
        {
          id: `err-${Date.now()}`,
          sender: 'system',
          content: `Error invoking agent loop: ${String(err)}`,
          timestamp: new Date().toISOString()
        }
      ]);
    } finally {
      setIsLoading(false);
    }
  };

  const handleApproval = async (id: string, approved: boolean) => {
    await submitApproval(id, approved);
    setPendingApprovals((prev) => prev.filter((a) => a.id !== id));
  };

  return (
    <div className="flex h-screen w-screen bg-[#090d16] text-slate-200">
      {/* Sidebar Navigation */}
      <aside className="w-64 border-r border-slate-800 bg-[#0e1424] flex flex-col justify-between">
        <div>
          {/* Logo & Title */}
          <div className="p-4 border-b border-slate-800/80 flex items-center space-x-3">
            <div className="p-2 bg-sky-500/10 border border-sky-500/30 rounded-lg text-sky-400">
              <ShieldAlert className="w-5 h-5" />
            </div>
            <div>
              <h1 className="text-sm font-bold text-white tracking-wide">INCIDENT AI</h1>
              <p className="text-[11px] text-slate-400 font-mono">MCP Triage Console</p>
            </div>
          </div>

          {/* Navigation Links */}
          <nav className="p-3 space-y-1">
            <button
              onClick={() => setActiveTab('chat')}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-lg text-xs font-medium transition ${
                activeTab === 'chat' 
                  ? 'bg-sky-500/15 text-sky-400 border border-sky-500/30' 
                  : 'text-slate-400 hover:bg-slate-800/60 hover:text-slate-200'
              }`}
            >
              <Bot className="w-4 h-4" />
              <span>Agent Investigation</span>
            </button>

            <button
              onClick={() => setActiveTab('approvals')}
              className={`w-full flex items-center justify-between px-3 py-2.5 rounded-lg text-xs font-medium transition ${
                activeTab === 'approvals' 
                  ? 'bg-amber-500/15 text-amber-400 border border-amber-500/30' 
                  : 'text-slate-400 hover:bg-slate-800/60 hover:text-slate-200'
              }`}
            >
              <div className="flex items-center space-x-3">
                <ShieldAlert className="w-4 h-4" />
                <span>HITL Approvals</span>
              </div>
              {pendingApprovals.length > 0 && (
                <span className="bg-amber-500/20 text-amber-300 px-1.5 py-0.5 rounded text-[10px] font-mono font-bold">
                  {pendingApprovals.length}
                </span>
              )}
            </button>

            <button
              onClick={() => setActiveTab('telemetry')}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-lg text-xs font-medium transition ${
                activeTab === 'telemetry' 
                  ? 'bg-sky-500/15 text-sky-400 border border-sky-500/30' 
                  : 'text-slate-400 hover:bg-slate-800/60 hover:text-slate-200'
              }`}
            >
              <FileText className="w-4 h-4" />
              <span>Telemetry Logs</span>
            </button>

            <button
              onClick={() => setActiveTab('deployments')}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-lg text-xs font-medium transition ${
                activeTab === 'deployments' 
                  ? 'bg-sky-500/15 text-sky-400 border border-sky-500/30' 
                  : 'text-slate-400 hover:bg-slate-800/60 hover:text-slate-200'
              }`}
            >
              <Clock className="w-4 h-4" />
              <span>Deployments</span>
            </button>

            <button
              onClick={() => setActiveTab('mcp')}
              className={`w-full flex items-center space-x-3 px-3 py-2.5 rounded-lg text-xs font-medium transition ${
                activeTab === 'mcp' 
                  ? 'bg-sky-500/15 text-sky-400 border border-sky-500/30' 
                  : 'text-slate-400 hover:bg-slate-800/60 hover:text-slate-200'
              }`}
            >
              <Server className="w-4 h-4" />
              <span>MCP Servers</span>
            </button>
          </nav>
        </div>

        {/* System Status Footer */}
        <div className="p-3 border-t border-slate-800 bg-[#090d16]/50">
          <div className="text-[11px] text-slate-400 font-mono mb-2 flex items-center justify-between">
            <span>MCP PROTOCOL</span>
            <span className="flex items-center space-x-1 text-emerald-400">
              <span className="w-2 h-2 rounded-full bg-emerald-500 animate-pulse"></span>
              <span>2.0 READY</span>
            </span>
          </div>
          <div className="text-[10px] text-slate-500 font-mono">
            <div>Auth: SHA-256 HITL Active</div>
            <div>Reference: 2026-11-20T12:00Z</div>
          </div>
        </div>
      </aside>

      {/* Main Content Area */}
      <main className="flex-1 flex flex-col overflow-hidden">
        {/* Top Header Bar */}
        <header className="h-14 border-b border-slate-800 bg-[#0e1424]/60 backdrop-blur flex items-center justify-between px-6">
          <div className="flex items-center space-x-3">
            <span className="text-sm font-semibold text-slate-200 uppercase tracking-wider">
              {activeTab === 'chat' && 'Autonomous Incident Investigation'}
              {activeTab === 'approvals' && 'Human-In-The-Loop (HITL) Gate'}
              {activeTab === 'telemetry' && 'Telemetry Log Explorer (mock_data/logs.json)'}
              {activeTab === 'deployments' && 'Deployment History & Rollback Status'}
              {activeTab === 'mcp' && 'Model Context Protocol (MCP) Infrastructure'}
            </span>
          </div>

          <div className="flex items-center space-x-3">
            <button 
              onClick={() => { loadMcpStatus(); loadLogs(); loadDeployments(); }}
              className="p-1.5 text-slate-400 hover:text-white rounded bg-slate-800/60 hover:bg-slate-800 border border-slate-700/60 text-xs flex items-center space-x-1.5"
            >
              <RefreshCw className="w-3.5 h-3.5" />
              <span>Refresh</span>
            </button>
          </div>
        </header>

        {/* Tab 1: Agent Chat & Investigation */}
        {activeTab === 'chat' && (
          <div className="flex-1 flex flex-col justify-between overflow-hidden">
            {/* Messages View */}
            <div className="flex-1 overflow-y-auto p-6 space-y-4">
              {messages.map((m) => (
                <div 
                  key={m.id} 
                  className={`flex items-start space-x-3 max-w-4xl ${
                    m.sender === 'user' ? 'ml-auto flex-row-reverse space-x-reverse' : ''
                  }`}
                >
                  <div className={`p-2 rounded-lg flex-shrink-0 ${
                    m.sender === 'user' 
                      ? 'bg-sky-600 text-white' 
                      : m.sender === 'agent' 
                        ? 'bg-slate-800 text-sky-400 border border-slate-700' 
                        : 'bg-emerald-950/80 text-emerald-400 border border-emerald-800/40'
                  }`}>
                    {m.sender === 'user' ? <User className="w-4 h-4" /> : m.sender === 'agent' ? <Bot className="w-4 h-4" /> : <Terminal className="w-4 h-4" />}
                  </div>

                  <div className={`flex flex-col space-y-1.5 ${m.sender === 'user' ? 'items-end' : ''}`}>
                    <div className="text-[11px] text-slate-400 font-mono">
                      {m.sender.toUpperCase()} • {new Date(m.timestamp).toLocaleTimeString()}
                    </div>
                    
                    <div className={`p-4 rounded-xl text-sm leading-relaxed ${
                      m.sender === 'user' 
                        ? 'bg-sky-600 text-white rounded-tr-none' 
                        : 'bg-[#111726] border border-slate-800 text-slate-200 rounded-tl-none shadow-md'
                    }`}>
                      <p className="whitespace-pre-wrap">{m.content}</p>

                      {/* Tool Calls Inspector */}
                      {m.toolCalls && m.toolCalls.length > 0 && (
                        <div className="mt-3 pt-3 border-t border-slate-700/60 space-y-2">
                          <div className="text-[11px] font-mono text-slate-400 uppercase tracking-wider flex items-center space-x-1.5">
                            <Layers className="w-3.5 h-3.5 text-sky-400" />
                            <span>MCP Tool Invocations</span>
                          </div>
                          {m.toolCalls.map((tc, idx) => (
                            <div key={idx} className="bg-[#090d16] border border-slate-800 rounded p-2.5 font-mono text-xs">
                              <div className="flex items-center justify-between text-sky-400 mb-1">
                                <span>{tc.server} / {tc.tool}</span>
                                <span className="text-[10px] px-1.5 py-0.5 rounded bg-emerald-500/10 text-emerald-400 border border-emerald-500/20">
                                  {tc.status}
                                </span>
                              </div>
                              <div className="text-slate-400 text-[11px]">Args: {JSON.stringify(tc.args)}</div>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  </div>
                </div>
              ))}
              <div ref={messagesEndRef} />
            </div>

            {/* Prompt Input Form */}
            <div className="p-4 border-t border-slate-800 bg-[#0e1424]/40">
              <form onSubmit={handleSend} className="flex items-center space-x-3">
                <input
                  type="text"
                  value={inputPrompt}
                  onChange={(e) => setInputPrompt(e.target.value)}
                  placeholder="Ask agent: e.g. 'Investigate payments-api errors and check if 2.14.0 caused it'..."
                  disabled={isLoading}
                  className="flex-1 bg-[#111726] border border-slate-800 rounded-lg px-4 py-3 text-sm text-white placeholder-slate-500 focus:outline-none focus:border-sky-500/60 transition"
                />
                <button
                  type="submit"
                  disabled={isLoading || !inputPrompt.trim()}
                  className="bg-sky-600 hover:bg-sky-500 disabled:opacity-50 text-white px-5 py-3 rounded-lg text-sm font-semibold flex items-center space-x-2 transition shadow-lg shadow-sky-600/20"
                >
                  <span>{isLoading ? 'Reasoning...' : 'Send'}</span>
                  <Send className="w-4 h-4" />
                </button>
              </form>
            </div>
          </div>
        )}

        {/* Tab 2: HITL Approval Gate */}
        {activeTab === 'approvals' && (
          <div className="p-6 overflow-y-auto space-y-4">
            <div className="p-4 rounded-xl bg-amber-500/10 border border-amber-500/30 flex items-start space-x-3">
              <AlertTriangle className="w-5 h-5 text-amber-400 flex-shrink-0 mt-0.5" />
              <div>
                <h3 className="text-sm font-bold text-amber-300">Human-In-The-Loop Action Gate</h3>
                <p className="text-xs text-amber-200/80 mt-1">
                  State-modifying actions (like creating production incident tickets) require cryptographically bound SHA-256 approval. 
                  Approving executes the action in SQLite; rejecting discards the proposed payload safely.
                </p>
              </div>
            </div>

            {pendingApprovals.length === 0 ? (
              <div className="text-center py-16 text-slate-500 font-mono text-sm">
                No state-modifying actions pending approval.
              </div>
            ) : (
              pendingApprovals.map((appr) => (
                <div key={appr.id} className="bg-[#111726] border border-slate-800 rounded-xl p-5 shadow-lg space-y-4">
                  <div className="flex items-center justify-between border-b border-slate-800/80 pb-3">
                    <div className="flex items-center space-x-2">
                      <span className="font-mono text-xs px-2 py-0.5 rounded bg-amber-500/20 text-amber-300 border border-amber-500/30">
                        {appr.tool}
                      </span>
                      <span className="text-xs text-slate-400 font-mono">Server: {appr.server}</span>
                    </div>
                    <span className="text-[11px] text-slate-500 font-mono">ID: {appr.id}</span>
                  </div>

                  <div>
                    <h4 className="text-sm font-semibold text-slate-200 mb-2">Proposal Payload:</h4>
                    <pre className="bg-[#090d16] border border-slate-800/80 p-3 rounded-lg text-xs font-mono text-sky-300 overflow-x-auto">
                      {JSON.stringify(appr.payload, null, 2)}
                    </pre>
                  </div>

                  <div className="text-xs font-mono text-slate-400 flex items-center space-x-2">
                    <span className="text-slate-500">SHA-256 BINDING:</span>
                    <span className="text-emerald-400">{appr.payloadHash}</span>
                  </div>

                  <div className="flex justify-end space-x-3 pt-2">
                    <button
                      onClick={() => handleApproval(appr.id, false)}
                      className="px-4 py-2 rounded-lg border border-red-500/40 text-red-400 hover:bg-red-500/10 text-xs font-semibold flex items-center space-x-1.5 transition"
                    >
                      <XCircle className="w-4 h-4" />
                      <span>Reject & Abort</span>
                    </button>
                    <button
                      onClick={() => handleApproval(appr.id, true)}
                      className="px-4 py-2 rounded-lg bg-emerald-600 hover:bg-emerald-500 text-white text-xs font-semibold flex items-center space-x-1.5 transition shadow-lg shadow-emerald-600/20"
                    >
                      <CheckCircle2 className="w-4 h-4" />
                      <span>Approve Execution</span>
                    </button>
                  </div>
                </div>
              ))
            )}
          </div>
        )}

        {/* Tab 3: Telemetry Log Explorer */}
        {activeTab === 'telemetry' && (
          <div className="p-6 flex-1 flex flex-col overflow-hidden">
            {/* Filters */}
            <div className="flex items-center space-x-4 mb-4">
              <div className="flex items-center space-x-2 text-xs font-mono">
                <span className="text-slate-400">Service:</span>
                <select 
                  value={logFilterService} 
                  onChange={(e) => setLogFilterService(e.target.value)}
                  className="bg-[#111726] border border-slate-800 rounded px-2.5 py-1.5 text-slate-200 text-xs"
                >
                  <option value="all">All Services</option>
                  <option value="payments-api">payments-api</option>
                  <option value="auth-service">auth-service</option>
                  <option value="notification-worker">notification-worker</option>
                </select>
              </div>

              <div className="flex items-center space-x-2 text-xs font-mono">
                <span className="text-slate-400">Severity:</span>
                <select 
                  value={logFilterSeverity} 
                  onChange={(e) => setLogFilterSeverity(e.target.value)}
                  className="bg-[#111726] border border-slate-800 rounded px-2.5 py-1.5 text-slate-200 text-xs"
                >
                  <option value="all">All Severities</option>
                  <option value="FATAL">FATAL</option>
                  <option value="ERROR">ERROR</option>
                  <option value="WARN">WARN</option>
                  <option value="INFO">INFO</option>
                </select>
              </div>

              <button 
                onClick={loadLogs}
                className="px-3 py-1.5 bg-sky-600/20 text-sky-400 border border-sky-500/30 rounded text-xs hover:bg-sky-600/30 transition"
              >
                Apply Filters
              </button>
            </div>

            {/* Logs Table */}
            <div className="flex-1 overflow-auto border border-slate-800 rounded-xl bg-[#111726]">
              <table className="w-full text-left text-xs font-mono">
                <thead className="bg-[#0e1424] text-slate-400 border-b border-slate-800 sticky top-0">
                  <tr>
                    <th className="p-3">Log ID</th>
                    <th className="p-3">Timestamp</th>
                    <th className="p-3">Service</th>
                    <th className="p-3">Severity</th>
                    <th className="p-3">Message</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-800/60">
                  {logs.length === 0 ? (
                    <tr>
                      <td colSpan={5} className="p-6 text-center text-slate-500">
                        No logs found matching filter.
                      </td>
                    </tr>
                  ) : (
                    logs.map((l) => (
                      <tr key={l.id} className="hover:bg-slate-800/30 transition">
                        <td className="p-3 text-slate-400">{l.id}</td>
                        <td className="p-3 text-slate-400 whitespace-nowrap">{l.timestamp}</td>
                        <td className="p-3 text-sky-400">{l.service}</td>
                        <td className="p-3">
                          <span className={`px-2 py-0.5 rounded text-[10px] font-bold ${
                            l.severity === 'FATAL' 
                              ? 'bg-rose-500/20 text-rose-300 border border-rose-500/40' 
                              : l.severity === 'ERROR' 
                                ? 'bg-red-500/20 text-red-300 border border-red-500/30'
                                : l.severity === 'WARN'
                                  ? 'bg-amber-500/20 text-amber-300 border border-amber-500/30'
                                  : 'bg-slate-700/40 text-slate-300'
                          }`}>
                            {l.severity}
                          </span>
                        </td>
                        <td className="p-3 text-slate-300 max-w-md truncate">{l.message}</td>
                      </tr>
                    ))
                  )}
                </tbody>
              </table>
            </div>
          </div>
        )}

        {/* Tab 4: Deployments */}
        {activeTab === 'deployments' && (
          <div className="p-6 overflow-auto">
            <div className="border border-slate-800 rounded-xl bg-[#111726]">
              <table className="w-full text-left text-xs font-mono">
                <thead className="bg-[#0e1424] text-slate-400 border-b border-slate-800">
                  <tr>
                    <th className="p-3">ID</th>
                    <th className="p-3">Service</th>
                    <th className="p-3">Version</th>
                    <th className="p-3">Deployed At</th>
                    <th className="p-3">Status</th>
                    <th className="p-3">Commit</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-800/60">
                  {deployments.map((d) => (
                    <tr key={d.deployment_id} className="hover:bg-slate-800/30">
                      <td className="p-3 text-slate-400">{d.deployment_id}</td>
                      <td className="p-3 text-sky-400">{d.service}</td>
                      <td className="p-3 text-slate-200 font-bold">{d.version}</td>
                      <td className="p-3 text-slate-400">{d.deployed_at}</td>
                      <td className="p-3">
                        <span className={`px-2 py-0.5 rounded text-[10px] font-bold ${
                          d.status === 'ACTIVE' 
                            ? 'bg-emerald-500/20 text-emerald-300 border border-emerald-500/30'
                            : 'bg-amber-500/20 text-amber-300 border border-amber-500/30'
                        }`}>
                          {d.status}
                        </span>
                      </td>
                      <td className="p-3 text-slate-500">{d.commit_hash}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}

        {/* Tab 5: MCP Servers */}
        {activeTab === 'mcp' && (
          <div className="p-6 space-y-4">
            <div className="grid grid-cols-2 gap-4">
              {servers.map((srv) => (
                <div key={srv.name} className="p-5 rounded-xl bg-[#111726] border border-slate-800 shadow-md space-y-3">
                  <div className="flex items-center justify-between">
                    <div className="flex items-center space-x-2">
                      <Server className="w-4 h-4 text-sky-400" />
                      <span className="font-bold text-sm text-white">{srv.name}</span>
                    </div>
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-emerald-500/20 text-emerald-400 border border-emerald-500/30">
                      {srv.status.toUpperCase()}
                    </span>
                  </div>

                  <div className="text-xs font-mono text-slate-400 space-y-1">
                    <div>Transport: <span className="text-slate-300">{srv.transport}</span></div>
                    <div>Registered Tools: <span className="text-sky-400 font-bold">{srv.toolsCount}</span></div>
                    <div>Resources: <span className="text-sky-400 font-bold">{srv.resourcesCount}</span></div>
                  </div>
                </div>
              ))}
            </div>
          </div>
        )}
      </main>
    </div>
  );
}
