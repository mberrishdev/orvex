import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { McpConnectionStatus } from "./McpStatus";

const providers = [
  { id: "codex", name: "Codex", mark: "◎", detail: "App, CLI & IDE", docs: "https://developers.openai.com/codex/mcp" },
  { id: "claude", name: "Claude Code", mark: "✳", detail: "Terminal & IDE", docs: "https://code.claude.com/docs/en/mcp" },
  { id: "copilot", name: "Copilot", mark: "⌘", detail: "GitHub Copilot CLI", docs: "https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers" },
  { id: "pi", name: "Pi", mark: "π", detail: "Coding agent", docs: "https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/mcp.md" },
  { id: "cursor", name: "Cursor", mark: "↗", detail: "Editor & CLI", docs: "https://cursor.com/docs/mcp" },
  { id: "gemini", name: "Gemini CLI", mark: "✦", detail: "Terminal", docs: "https://geminicli.com/docs/tools/mcp-server/" },
  { id: "other", name: "Other client", mark: "+", detail: "Manual MCP setup", docs: "https://modelcontextprotocol.io/docs/develop/connect-local-servers" },
];
type Setup = { provider: string; path: string; config: string; configured: boolean; conflict: boolean };

export function McpSetup({ status, statusError, refresh, onClose }: {
  status: McpConnectionStatus | null; statusError: string | null; refresh: () => Promise<void>; onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [providerId, setProviderId] = useState("codex");
  const [setup, setSetup] = useState<Setup | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const provider = providers.find(p => p.id === providerId)!;
  useEffect(() => { dialog.current?.showModal(); }, []);
  useEffect(() => {
    let current = true;
    setLoading(true); setSetup(null); setError(null); setMessage(null); setCopied(false);
    invoke<Setup>("get_mcp_setup", { provider: providerId }).then(result => { if (current) setSetup(result); })
      .catch(reason => { if (current) setError(String(reason)); })
      .finally(() => { if (current) setLoading(false); });
    return () => { current = false; };
  }, [providerId]);
  async function install() {
    setBusy(true); setError(null); setMessage(null);
    try {
      setSetup(await invoke<Setup>("install_mcp_setup", { provider: providerId }));
      setMessage(`Orvex added to ${provider.name}. Restart your client or reload its MCP servers.`);
    } catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  }
  async function copy() {
    if (!setup) return;
    try { await navigator.clipboard.writeText(setup.config); setCopied(true); }
    catch { setError("Could not access the clipboard. Select and copy the configuration below."); }
  }
  const live = status?.clients.some(client => providerId !== "other" && (providerId === "pi" ? /(^|[^a-z])pi([^a-z]|$)/i.test(client.name) : client.name.toLowerCase().includes(providerId))) ?? false;
  return <dialog ref={dialog} className="workspace-dialog mcp-setup-dialog" aria-labelledby="mcp-setup-title" onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="mcp-setup-top"><span className="eyebrow">GET CONNECTED</span><button type="button" className="mcp-close" aria-label="Close setup" disabled={busy} onClick={onClose}>×</button></div>
    <h2 id="mcp-setup-title">Your AI, meet your workspaces.</h2>
    <p>Add Orvex to the AI client you use on this computer. Your agent can discover workspaces, inspect their status, and run tools inside them.</p>
    <div className="mcp-provider-grid" role="group" aria-label="Choose your AI client">
      {providers.map(p => <button type="button" className={`mcp-provider ${p.id === providerId ? "selected" : ""}`} aria-pressed={p.id === providerId} key={p.id} disabled={busy} onClick={() => setProviderId(p.id)}><span aria-hidden="true">{p.mark}</span><strong>{p.name}</strong><small>{p.detail}</small></button>)}
    </div>
    <section className="mcp-setup-detail" aria-label={`${provider.name} setup`} aria-busy={loading || busy}>
      <div className="mcp-step"><span>1</span><div><h3>Add Orvex to {provider.name}</h3>
        {loading ? <p role="status">Reading setup…</p> : setup && <>
          {providerId === "other" ? <p>Merge this entry into your client’s MCP settings. Use a local stdio server; some clients use a “servers” key instead of “mcpServers”.</p> : <><p>{setup.configured ? "Configuration saved." : "Adds a user-level MCP server named “orvex”. Other settings are preserved; existing files are backed up beside the original."}</p><code className="mcp-config-path">{setup.path}</code></>}
          {setup.conflict && <p className="runtime-feedback-error" role="alert">An Orvex entry already exists with different settings. Review it manually using the configuration below.</p>}
          {providerId === "pi" && <p>Requires a Pi version with built-in MCP support. Older versions may need an MCP extension.</p>}
          <div className="mcp-setup-actions">
            {providerId !== "other" && <button className="primary-button" type="button" disabled={busy || setup.configured || setup.conflict} onClick={() => void install()}>{busy ? "Adding…" : setup.configured ? "Added to client ✓" : `Add to ${provider.name}`}</button>}
            <button className="secondary-button" type="button" onClick={() => void copy()}>{copied ? "Copied ✓" : "Copy configuration"}</button>
          </div>
          <details key={providerId}><summary>View configuration</summary><pre tabIndex={0}>{setup.config}</pre></details>
        </>}
      </div></div>
      <div className="mcp-step"><span>2</span><div><h3>Open your client & check the connection</h3><p>Restart the client or reload its MCP servers, then enable Orvex if asked. Try: <b>“List my Orvex workspaces.”</b></p>
        <div className="mcp-connection-check"><span className={`runtime-dot ${live ? "ready" : ""}`}/><span>{statusError ? "Could not read live connections. Try checking again." : live ? `${provider.name} is connected` : "Waiting for a live MCP connection"}</span><button type="button" className="secondary-button" disabled={busy} onClick={() => void refresh()}>Check again</button></div>
        <small>Saving configuration does not start a session. Live connections appear automatically when your client connects.</small>
      </div></div>
    </section>
    {message && <p className="mcp-setup-message" role="status">{message}</p>}
    {error && <p className="runtime-feedback-error" role="alert">{error}</p>}
    <div className="mcp-setup-footer"><button className="mcp-docs" type="button" onClick={() => void openUrl(provider.docs).catch(reason => setError(String(reason)))}>Client setup guide ↗</button><button className="secondary-button" type="button" disabled={busy} onClick={onClose}>Done</button></div>
    <small className="mcp-permissions">MCP gives your agent access to Orvex workspace tools. Network operations still require explicit grants. Keep the app installed at this location so the client can launch its server.</small>
  </dialog>;
}
