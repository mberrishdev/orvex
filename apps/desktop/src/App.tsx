import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type AppInfo = { name: string; version: string; platform: string };
type Workspace = { id: string; name: string; branch: string; status: string };
type LimaStatus = { installed: boolean; version: string | null; installSupported: boolean; installer: string | null };

function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [lima, setLima] = useState<LimaStatus | null>(null);
  const [installing, setInstalling] = useState(false);
  const [runtimeMessage, setRuntimeMessage] = useState<string | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([
      invoke<AppInfo>("get_app_info"),
      invoke<Workspace[]>("list_workspaces"),
      invoke<LimaStatus>("get_lima_status"),
    ])
      .then(([appInfo, items, limaStatus]) => { setInfo(appInfo); setWorkspaces(items); setLima(limaStatus); })
      .catch((reason: unknown) => setError(String(reason)));
  }, []);

  async function installLima() {
    setInstalling(true);
    setRuntimeError(null);
    setRuntimeMessage("Installing Lima with Homebrew. This downloads software and may take a few minutes.");
    try {
      const status = await invoke<LimaStatus>("install_lima");
      setLima(status);
      setRuntimeMessage("Lima is installed and ready. No VM has been started.");
    } catch (reason) {
      setRuntimeMessage(null);
      setRuntimeError(String(reason));
    } finally {
      setInstalling(false);
    }
  }

  async function refreshLima() {
    try {
      setLima(await invoke<LimaStatus>("get_lima_status"));
      setRuntimeError(null);
      setRuntimeMessage("Runtime status refreshed.");
    } catch (reason) {
      setRuntimeError(String(reason));
    }
  }

  return <main className="shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark">◈</span> orvex</div>
      <div className="side-label">WORKSPACES</div>
      <button className="nav-item selected"><span>▦</span> All tasks <span className="count">{workspaces.length}</span></button>
      <button className="nav-item"><span>◷</span> Recent</button>
      <div className="side-label projects-label">PROJECTS</div>
      <div className="empty-projects">Your projects will appear here.</div>
      <div className="sidebar-bottom"><div className="runtime-dot"/><div><strong>{info?.platform ?? "Connecting…"}</strong><small>Local controller</small></div><span className="connected">●</span></div>
    </aside>
    <section className="content">
      <header className="topbar"><span>Workspace manager</span><button className="icon-button" aria-label="Settings">⚙</button></header>
      <div className="page-heading"><div><div className="eyebrow">YOUR DEVELOPMENT ENVIRONMENTS</div><h1>Workspaces</h1><p>Give every task its own code, services, and agent session.</p></div><button className="primary-button" disabled><span>＋</span> New workspace</button></div>
      {error && <div className="notice error">Controller connection failed: {error}</div>}
      <section className={`runtime-card ${lima?.installed ? "runtime-ready" : "runtime-needs-setup"}`} aria-live="polite">
        <div className="runtime-icon">{lima?.installed ? "✓" : "⌁"}</div>
        <div className="runtime-copy">
          <div className="runtime-title-row"><h2>Linux runtime</h2><span className={`runtime-badge ${lima?.installed ? "ready" : "needs-setup"}`}>{lima ? (lima.installed ? "READY" : "SETUP REQUIRED") : "CHECKING"}</span></div>
          <p>{lima?.installed ? `Lima is installed${lima.version ? ` · ${lima.version}` : ""}. VM creation is not enabled yet.` : "Lima provides lightweight Linux virtual machines on macOS. Installing it requires an internet connection; Orvex will not start a VM during setup."}</p>
          {runtimeMessage && <small className="runtime-feedback">{runtimeMessage}</small>}
          {runtimeError && <small className="runtime-feedback runtime-feedback-error">{runtimeError}</small>}
          {lima && !lima.installed && lima.installSupported && <small className="runtime-install-note">Install method: Homebrew ({lima.installer})</small>}
          {lima && !lima.installed && !lima.installSupported && <small className="runtime-install-note">Install Homebrew to enable the in-app Lima installer.</small>}
        </div>
        {lima?.installed
          ? <button className="secondary-button" onClick={refreshLima}>Check again</button>
          : <button className="primary-button" onClick={installLima} disabled={!lima?.installSupported || installing || !lima}>
              {installing ? <><span className="spinner"/> Installing…</> : "Install Lima"}
            </button>}
      </section>
      {workspaces.length === 0 ? <div className="empty-card">
        <div className="empty-illustration"><div className="orbit orbit-one"/><div className="orbit orbit-two"/><div className="cube">◈</div><span className="spark spark-one">✳</span><span className="spark spark-two">✦</span></div>
        <h2>Nothing here yet</h2><p>Create a workspace from a repository to give a task its own checkout,<br className="desktop-break"/> services, and place to run your coding agent.</p>
        <button className="primary-button" disabled>＋ &nbsp;Create your first workspace</button><span className="coming-soon">Workspace provisioning is the next milestone</span>
      </div> : <div className="workspace-list">{workspaces.map((w) => <article className="workspace-card" key={w.id}><div><b>{w.name}</b><p>{w.branch}</p></div><span className="status">{w.status}</span></article>)}</div>}
      <footer className="footer"><span>Local-first by design</span><span>{info ? `${info.name} · ${info.version}` : "Starting controller…"}</span></footer>
    </section>
  </main>;
}
export default App;
