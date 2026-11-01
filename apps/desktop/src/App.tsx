import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import "./App.css";

type AppInfo = { name: string; version: string; platform: string };
type Workspace = { id: string; name: string; branch: string; status: string };

function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([invoke<AppInfo>("get_app_info"), invoke<Workspace[]>("list_workspaces")])
      .then(([appInfo, items]) => { setInfo(appInfo); setWorkspaces(items); })
      .catch((reason: unknown) => setError(String(reason)));
  }, []);

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
