export type McpConnectionStatus = {
  clients: { id: string; name: string; version: string; processId: number; connectedAt: number }[];
  ignoredRecords: number;
};

export function McpStatus({ status, error }: { status: McpConnectionStatus | null; error: string | null }) {
  const clients = status?.clients ?? [];
  const name = (raw: string) => /codex/i.test(raw) ? "Codex" : /copilot/i.test(raw) ? "Copilot" : /claude/i.test(raw) ? "Claude" : raw;
  return <section className="runtime-card mcp-card" aria-label="MCP agent connections" aria-live="polite">
    <div className="runtime-icon" aria-hidden="true">⌁</div>
    <div className="runtime-copy">
      <div className="runtime-title-row"><h2>Agent connections</h2><span className={`runtime-badge ${clients.length ? "ready" : "needs-setup"}`}>{clients.length ? `${clients.length} LIVE` : "MCP"}</span></div>
      {error ? <p className="runtime-feedback-error">Unable to read connections. Use Refresh to retry.</p>
        : clients.length ? <div className="mcp-clients">{clients.map(client => <span className="mcp-client" key={client.id} title={`${client.name} · ${client.version} · process ${client.processId}`}><i className="local-indicator"/>{name(client.name)}<small>{client.version === "Unknown" ? "" : client.version}</small></span>)}</div>
        : <p>{status ? "No agent connected. Enable Orvex MCP in Codex or Copilot." : "Checking MCP connections…"}</p>}
      {clients.length > 0 && <small className="mcp-note">Live MCP sessions · provider sign-in is not checked.</small>}
    </div>
  </section>;
}
