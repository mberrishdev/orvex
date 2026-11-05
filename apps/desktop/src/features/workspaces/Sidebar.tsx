import { BrandMark } from "../../components/BrandMark";
import type { Workspace } from "./types";

type Props = {
  workspaces: Workspace[];
  runningCount: number;
  filter: "all" | "running";
  onFilter: (filter: "all" | "running") => void;
  page: "workspaces" | "settings";
  onSettings: () => void;
  platform?: string;
  runtimeReady: boolean;
};

export function Sidebar({ workspaces, runningCount, filter, onFilter, page, onSettings, platform, runtimeReady }: Props) {
  return <aside className="sidebar">
    <div className="window-drag-region" data-tauri-drag-region aria-hidden="true"/>
    <div className="brand"><BrandMark /><span>orvex<span className="brand-caption">A place to make things.</span></span></div>
    <div className="side-label">LIBRARY</div>
    <nav aria-label="Workspace navigation">
      <button className={`nav-item ${page === "workspaces" && filter === "all" ? "selected" : ""}`} aria-current={page === "workspaces" && filter === "all" ? "page" : undefined} onClick={() => onFilter("all")}><span aria-hidden="true">▦</span> All workspaces <span className="count">{workspaces.length}</span></button>
      <button className={`nav-item ${page === "workspaces" && filter === "running" ? "selected" : ""}`} aria-current={page === "workspaces" && filter === "running" ? "page" : undefined} onClick={() => onFilter("running")}><span aria-hidden="true">◉</span> Running <span className="count">{runningCount}</span></button>
    </nav>
    <div className="side-label projects-label">YOUR WORKSPACES</div>
    <div className="sidebar-workspaces">{workspaces.length ? workspaces.map(workspace => <button className="workspace-shortcut" key={workspace.id} onClick={() => {
      onFilter("all");
      requestAnimationFrame(() => document.getElementById(`workspace-${workspace.id}`)?.scrollIntoView({ behavior: "smooth", block: "center" }));
    }}><span aria-hidden="true">◇</span><span>{workspace.name}</span></button>) : <p className="empty-projects">Your next idea starts here.</p>}</div>
    <div className="sidebar-note"><span aria-hidden="true">⌘</span><p>Your code stays local.<br/>Your environments stay separate.</p></div>
    <nav aria-label="App navigation"><button type="button" className={`nav-item ${page === "settings" ? "selected" : ""}`} aria-label="Settings" aria-current={page === "settings" ? "page" : undefined} onClick={onSettings}><span aria-hidden="true">⚙</span> Settings</button></nav>
    <div className="sidebar-bottom"><span className={`runtime-dot ${runtimeReady ? "ready" : ""}`}/><div><strong>{runtimeReady ? "Ready when you are" : "Let’s get set up"}</strong><small>{platform === "macos" ? "macOS" : platform ?? "Connecting"} · Local runtime</small></div></div>
  </aside>;
}
