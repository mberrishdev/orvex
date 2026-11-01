import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type AppInfo = { name: string; version: string; platform: string };
type Workspace = { id: string; name: string; branch: string; status: string };
type LimaStatus = { installed: boolean; version: string | null; installSupported: boolean; installer: string | null };
type VmStatus = { state: "notCreated" | "running" | "stopped" | string; instanceName: string; imageBytes: number | null };
type VmProgress = { phase: string; message: string; downloadedBytes?: number | null };

const vmStages = [
  { phase: "checking", label: "Check" },
  { phase: "downloading", label: "Download" },
  { phase: "preparing", label: "Prepare" },
  { phase: "booting", label: "Boot" },
  { phase: "provisioning", label: "Configure" },
  { phase: "ready", label: "Ready" },
];

function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [lima, setLima] = useState<LimaStatus | null>(null);
  const [vm, setVm] = useState<VmStatus | null>(null);
  const [vmAction, setVmAction] = useState<string | null>(null);
  const [vmProgress, setVmProgress] = useState<VmProgress | null>(null);
  const [vmStartedAt, setVmStartedAt] = useState<number | null>(null);
  const [vmElapsedSeconds, setVmElapsedSeconds] = useState(0);
  const [vmOutput, setVmOutput] = useState<string | null>(null);
  const [vmError, setVmError] = useState<string | null>(null);
  const [installing, setInstalling] = useState(false);
  const [runtimeMessage, setRuntimeMessage] = useState<string | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    Promise.all([
      invoke<AppInfo>("get_app_info"),
      invoke<Workspace[]>("list_workspaces"),
      invoke<LimaStatus>("get_lima_status"),
      invoke<VmStatus>("get_vm_status"),
    ])
      .then(([appInfo, items, limaStatus, vmStatus]) => { setInfo(appInfo); setWorkspaces(items); setLima(limaStatus); setVm(vmStatus); })
      .catch((reason: unknown) => setError(String(reason)));
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<VmProgress>("vm-start-progress", ({ payload }) => setVmProgress((current) =>
      payload.phase === "downloading" && payload.downloadedBytes == null && current?.phase === "downloading"
        ? { ...payload, downloadedBytes: current.downloadedBytes }
        : payload,
    ))
      .then((stopListening) => {
        if (disposed) stopListening();
        else unlisten = stopListening;
      })
      .catch((reason: unknown) => setVmError(String(reason)));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (vmAction !== "start" || vmStartedAt === null) return;
    const timer = window.setInterval(() => {
      setVmElapsedSeconds(Math.floor((Date.now() - vmStartedAt) / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [vmAction, vmStartedAt]);

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

  async function runVmAction(action: "start" | "check" | "stop" | "delete") {
    const commands = { start: "start_poc_vm", check: "run_poc_check", stop: "stop_poc_vm", delete: "delete_poc_vm" } as const;
    setVmAction(action);
    setVmError(null);
    if (action === "start") {
      setVmOutput(null);
      setVmElapsedSeconds(0);
      setVmStartedAt(Date.now());
      setVmProgress({ phase: "checking", message: "Preparing the Orvex Linux VM…" });
    }
    try {
      if (action === "check") {
        setVmOutput(await invoke<string>(commands.check));
      } else {
        const next = await invoke<VmStatus>(commands[action]);
        setVm(next);
        if (action === "start") {
          setVmProgress({ phase: "ready", message: "Linux VM is ready." });
        }
        if (action === "delete") setVmOutput(null);
      }
    } catch (reason) {
      setVmError(String(reason));
      if (action === "start") {
        setVmProgress({ phase: "failed", message: "VM startup stopped or failed." });
      }
    } finally {
      setVmAction(null);
      if (action === "start") setVmStartedAt(null);
    }
  }

  const currentVmStage = Math.max(0, vmStages.findIndex((stage) => stage.phase === vmProgress?.phase));
  const elapsedLabel = `${Math.floor(vmElapsedSeconds / 60)}:${String(vmElapsedSeconds % 60).padStart(2, "0")}`;
  const downloadLabel = vmProgress?.downloadedBytes == null
    ? "First startup downloads and prepares Ubuntu; it can take a few minutes."
    : `${Math.floor(vmProgress.downloadedBytes / 1024).toLocaleString()} KB downloaded · Lima does not report the total image size.`;

  async function deleteVm() {
    if (window.confirm("Delete the Orvex proof-of-concept VM and its disk?")) {
      await runVmAction("delete");
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
      <section className="vm-card" aria-live="polite">
        <div className="vm-heading">
          <div><div className="eyebrow">RUNTIME PROOF OF CONCEPT</div><h2>Linux VM</h2></div>
          <span className={`runtime-badge ${vm?.state === "running" ? "ready" : "needs-setup"}`}>{vm?.state?.replace(/[A-Z]/g, (letter) => ` ${letter}`).toUpperCase() ?? "CHECKING"}</span>
        </div>
        <p className="vm-description">Start an app-owned Lima VM, run a fixed <code>uname -a</code> check, then stop or delete it. Host folders are not mounted and containerd is off.</p>
        <p className="vm-image-cache">{vm?.imageBytes != null ? `Ubuntu image cached · ${Math.floor(vm.imageBytes / 1024).toLocaleString()} KB` : "Ubuntu image not downloaded yet"}</p>
        <div className="network-consent"><strong>Network access is enabled for this proof.</strong> The guest can reach the internet and services on your Mac’s loopback address. Starting it is always a manual action.</div>
        {vmError && <div className="vm-error">{vmError}</div>}
        {vmOutput && <pre className="vm-output">{vmOutput}</pre>}
        {vmAction === "start" && <div className="vm-progress" role="status" aria-live="polite">
          <div className="vm-progress-current"><span className="spinner"/><div><strong>{vmProgress?.message ?? "Starting Linux VM…"}</strong><small>{downloadLabel}</small></div><time>{elapsedLabel}</time></div>
          <div className="vm-progress-track" aria-hidden="true"><span/></div>
          <ol className="vm-stage-list">{vmStages.map((stage, index) => <li className={index < currentVmStage ? "complete" : index === currentVmStage ? "active" : ""} key={stage.phase}><span className="vm-stage-mark">{index < currentVmStage ? "✓" : index + 1}</span><span>{stage.label}</span></li>)}</ol>
        </div>}
        <div className="vm-actions">
          {vm?.state !== "running" && <button className="primary-button" disabled={!lima?.installed || !vm || vmAction !== null} onClick={() => runVmAction("start")}>
            {vmAction === "start" ? <><span className="spinner"/> Starting VM…</> : vm?.state === "stopped" ? "Resume VM" : "Start VM"}
          </button>}
          {vm?.state === "running" && <>
            <button className="primary-button" disabled={vmAction !== null} onClick={() => runVmAction("check")}>
              {vmAction === "check" ? <><span className="spinner"/> Running…</> : "Run uname -a"}
            </button>
            <button className="secondary-button" disabled={vmAction !== null} onClick={() => runVmAction("stop")}>{vmAction === "stop" ? "Stopping…" : "Stop VM"}</button>
          </>}
          {vm && vm.state !== "notCreated" && <button className="secondary-button" disabled={vmAction !== null} onClick={deleteVm}>{vmAction === "delete" ? "Deleting…" : "Delete VM"}</button>}
        </div>
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
