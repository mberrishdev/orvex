import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type AppInfo = { name: string; version: string; platform: string };
type Workspace = { id: string; name: string; branch: string; status: string; path: string; repoUrl: string | null; vmName: string | null };
type LimaStatus = { installed: boolean; version: string | null; installSupported: boolean; installer: string | null };
type VmStatus = { state: "notCreated" | "running" | "stopped" | string; instanceName: string; imageBytes: number | null };
type VmProgress = { phase: string; message: string; downloadedBytes?: number | null; workspaceId?: string };

const vmStages = [
  { phase: "checking", label: "Check" },
  { phase: "downloading", label: "Download" },
  { phase: "preparing", label: "Prepare" },
  { phase: "booting", label: "Boot" },
  { phase: "provisioning", label: "Configure" },
  { phase: "ready", label: "Ready" },
];

function workspaceSlugPreview(name: string) {
  return name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}

function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [workspaceVms, setWorkspaceVms] = useState<Record<string, VmStatus>>({});
  const [workspaceVmProgress, setWorkspaceVmProgress] = useState<Record<string, VmProgress>>({});
  const [workspaceVmAction, setWorkspaceVmAction] = useState<string | null>(null);
  const [workspaceVmError, setWorkspaceVmError] = useState<string | null>(null);
  const [lima, setLima] = useState<LimaStatus | null>(null);
  const [vm, setVm] = useState<VmStatus | null>(null);
  const [vmAction, setVmAction] = useState<string | null>(null);
  const [vmProgress, setVmProgress] = useState<VmProgress | null>(null);
  const [vmStartedAt, setVmStartedAt] = useState<number | null>(null);
  const [vmElapsedSeconds, setVmElapsedSeconds] = useState(0);
  const [vmOutput, setVmOutput] = useState<string | null>(null);
  const [vmError, setVmError] = useState<string | null>(null);
  const [workspaceDialogOpen, setWorkspaceDialogOpen] = useState(false);
  const [workspaceName, setWorkspaceName] = useState("");
  const [repoUrl, setRepoUrl] = useState("");
  const [allowVmNetwork, setAllowVmNetwork] = useState(false);
  const [workspaceCreating, setWorkspaceCreating] = useState(false);
  const [workspaceError, setWorkspaceError] = useState<string | null>(null);
  const [terminalAction, setTerminalAction] = useState<string | null>(null);
  const [terminalError, setTerminalError] = useState<string | null>(null);
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
      .then(async ([appInfo, items, limaStatus, vmStatus]) => {
        setInfo(appInfo);
        setWorkspaces(items);
        setLima(limaStatus);
        setVm(vmStatus);
        const statuses = await Promise.all(items.map(async (workspace) => {
          try {
            return [workspace.id, await invoke<VmStatus>("get_workspace_vm_status", { workspaceId: workspace.id })] as const;
          } catch {
            return [workspace.id, null] as const;
          }
        }));
        setWorkspaceVms(Object.fromEntries(statuses.filter((entry): entry is readonly [string, VmStatus] => entry[1] !== null)));
      })
      .catch((reason: unknown) => setError(String(reason)));
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<VmProgress>("vm-start-progress", ({ payload }) => {
      if (payload.workspaceId) {
        setWorkspaceVmProgress((current) => ({ ...current, [payload.workspaceId as string]: payload }));
        return;
      }
      setVmProgress((current) => payload.phase === "downloading" && payload.downloadedBytes == null && current?.phase === "downloading"
        ? { ...payload, downloadedBytes: current.downloadedBytes }
        : payload);
    })
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

  function openWorkspaceDialog() {
    setWorkspaceName("");
    setRepoUrl("");
    setAllowVmNetwork(false);
    setWorkspaceError(null);
    setWorkspaceDialogOpen(true);
  }

  async function createWorkspace() {
    if (!lima?.installed) {
      setWorkspaceError("Install Lima before creating a workspace VM.");
      return;
    }
    setWorkspaceCreating(true);
    setWorkspaceError(null);
    try {
      const workspace = await invoke<Workspace>("create_workspace", {
        name: workspaceName,
        repoUrl: repoUrl.trim() || null,
        allowNetwork: allowVmNetwork,
      });
      setWorkspaces((current) => [workspace, ...current]);
      const workspaceVm: VmStatus = { state: "running", instanceName: workspace.vmName ?? "", imageBytes: null };
      setWorkspaceVms((current) => ({ ...current, [workspace.id]: workspaceVm }));
      setWorkspaceDialogOpen(false);
      setWorkspaceName("");
      setRepoUrl("");
      setAllowVmNetwork(false);
      setWorkspaceVmProgress((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
    } catch (reason) {
      setWorkspaceError(String(reason));
    } finally {
      setWorkspaceCreating(false);
    }
  }

  async function runWorkspaceVmAction(workspace: Workspace, action: "start" | "stop") {
    const actionKey = `${workspace.id}:${action}`;
    setWorkspaceVmAction(actionKey);
    setWorkspaceVmError(null);
    try {
      const status = await invoke<VmStatus>(action === "start" ? "start_workspace_vm" : "stop_workspace_vm", { workspaceId: workspace.id });
      setWorkspaceVms((current) => ({ ...current, [workspace.id]: status }));
    } catch (reason) {
      setWorkspaceVmError(`${workspace.name}: ${String(reason)}`);
    } finally {
      setWorkspaceVmAction(null);
    }
  }

  async function deleteWorkspace(workspace: Workspace) {
    if (!window.confirm(`Delete “${workspace.name}” and permanently remove its dedicated VM and files?`)) return;
    setWorkspaceVmAction(`${workspace.id}:delete`);
    setWorkspaceVmError(null);
    try {
      await invoke("delete_workspace", { workspaceId: workspace.id });
      setWorkspaces((current) => current.filter((item) => item.id !== workspace.id));
      setWorkspaceVms((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
    } catch (reason) {
      setWorkspaceVmError(`${workspace.name}: ${String(reason)}`);
    } finally {
      setWorkspaceVmAction(null);
    }
  }

  async function openWorkspaceTerminal(workspace: Workspace, networkEnabled: boolean) {
    if (networkEnabled && !window.confirm(
      `Open a terminal for “${workspace.name}” with the Linux VM's current network access?\n\nThis allows the shell and commands it starts to access the internet and reachable host or LAN services.`,
    )) return;
    setTerminalAction(`${workspace.id}:${networkEnabled ? "online" : "offline"}`);
    setTerminalError(null);
    try {
      await invoke("open_workspace_terminal", { workspaceId: workspace.id, networkEnabled });
    } catch (reason) {
      setTerminalError(`${workspace.name}: ${String(reason)}`);
    } finally {
      setTerminalAction(null);
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
        setWorkspaceVms((current) => {
          const legacyIds = workspaces.filter((workspace) => !workspace.vmName).map((workspace) => workspace.id);
          if (action === "delete") {
            const remaining = { ...current };
            legacyIds.forEach((id) => delete remaining[id]);
            return remaining;
          }
          return Object.fromEntries([...Object.entries(current), ...legacyIds.map((id) => [id, next] as const)]);
        });
        if (action === "delete") setWorkspaces((current) => current.filter((workspace) => workspace.vmName));
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
    if (window.confirm("Delete the legacy shared VM and permanently erase all older workspaces stored on it? This cannot be undone.")) {
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
      <div className="page-heading"><div><div className="eyebrow">YOUR DEVELOPMENT ENVIRONMENTS</div><h1>Workspaces</h1><p>Give every task its own code, services, and agent session.</p></div><button className="primary-button" onClick={openWorkspaceDialog}><span>＋</span> New workspace</button></div>
      {error && <div className="notice error">Controller connection failed: {error}</div>}
      <section className={`runtime-card ${lima?.installed ? "runtime-ready" : "runtime-needs-setup"}`} aria-live="polite">
        <div className="runtime-icon">{lima?.installed ? "✓" : "⌁"}</div>
        <div className="runtime-copy">
          <div className="runtime-title-row"><h2>Linux runtime</h2><span className={`runtime-badge ${lima?.installed ? "ready" : "needs-setup"}`}>{lima ? (lima.installed ? "READY" : "SETUP REQUIRED") : "CHECKING"}</span></div>
          <p>{lima?.installed ? `Lima is installed${lima.version ? ` · ${lima.version}` : ""}. Each new workspace gets its own Linux VM.` : "Lima provides lightweight Linux virtual machines on macOS. Installing it requires an internet connection; Orvex will not start a VM during setup."}</p>
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
      {workspaces.some((workspace) => !workspace.vmName) && <section className="vm-card" aria-live="polite">
        <div className="vm-heading">
          <div><div className="eyebrow">EXISTING WORKSPACES</div><h2>Legacy shared VM</h2></div>
          <span className={`runtime-badge ${vm?.state === "running" ? "ready" : "needs-setup"}`}>{vm?.state?.replace(/[A-Z]/g, (letter) => ` ${letter}`).toUpperCase() ?? "CHECKING"}</span>
        </div>
        <p className="vm-description">These older workspaces still share one VM. New workspaces get separate VMs. Stopping or deleting this VM affects every older workspace.</p>
        <p className="vm-image-cache">{vm?.imageBytes != null ? `Ubuntu image cached · ${Math.floor(vm.imageBytes / 1024).toLocaleString()} KB` : "Ubuntu image not downloaded yet"}</p>
        <div className="network-consent"><strong>This shared VM has network access.</strong> Offline terminals use a Linux network namespace; other guest processes remain networked.</div>
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
      </section>}
      {terminalError && <div className="notice error" role="alert">{terminalError}</div>}
      {workspaceVmError && <div className="notice error" role="alert">{workspaceVmError}</div>}
      {workspaces.length === 0 ? <div className="empty-card">
        <div className="empty-illustration"><div className="orbit orbit-one"/><div className="orbit orbit-two"/><div className="cube">◈</div><span className="spark spark-one">✳</span><span className="spark spark-two">✦</span></div>
        <h2>Nothing here yet</h2><p>Create a workspace with its own Linux VM.<br className="desktop-break"/> Its code and lifecycle stay separate from other workspaces.</p>
        <button className="primary-button" onClick={openWorkspaceDialog}>＋ &nbsp;Create your first workspace</button><span className="coming-soon">Each workspace uses a separate Linux VM</span>
      </div> : <div className="workspace-list">{workspaces.map((w) => {
        const workspaceVm = workspaceVms[w.id];
        const state = workspaceVm?.state ?? "checking";
        const isLegacy = !w.vmName;
        const vmActionBusy = workspaceVmAction?.startsWith(`${w.id}:`) ?? false;
        return <article className="workspace-card" key={w.id}>
          <div className="workspace-details"><b>{w.name}</b><p>{w.branch}</p><small className="workspace-path">{w.path}</small>{w.repoUrl && <small className="workspace-path">{w.repoUrl}</small>}
            <span className={`runtime-badge ${state === "running" ? "ready" : "needs-setup"}`}>{isLegacy ? "SHARED LEGACY VM" : `WORKSPACE VM · ${state.toUpperCase()}`}</span>
          </div>
          <div className="workspace-controls"><span className="status">{w.status}</span>
            {!isLegacy && <div className="workspace-terminal-actions">
              {state !== "running" && <button className="primary-button" disabled={!lima?.installed || vmActionBusy || workspaceVmAction !== null} onClick={() => void runWorkspaceVmAction(w, "start")}>{workspaceVmAction === `${w.id}:start` ? <><span className="spinner"/> Starting…</> : state === "stopped" ? "Resume VM" : "Start VM"}</button>}
              {state === "running" && <button className="secondary-button" disabled={vmActionBusy || workspaceVmAction !== null} onClick={() => void runWorkspaceVmAction(w, "stop")}>{workspaceVmAction === `${w.id}:stop` ? "Stopping…" : "Stop VM"}</button>}
              <button className="danger-button" disabled={vmActionBusy || workspaceVmAction !== null} onClick={() => void deleteWorkspace(w)}>{workspaceVmAction === `${w.id}:delete` ? "Deleting…" : "Delete workspace"}</button>
            </div>}
            {!isLegacy && workspaceVmAction === `${w.id}:start` && <small className="runtime-feedback">{workspaceVmProgress[w.id]?.message ?? "Starting this workspace's VM…"}</small>}
            <div className="workspace-terminal-actions"><button className="secondary-button" disabled={state !== "running" || terminalAction !== null || (isLegacy && vm?.state !== "running")} onClick={() => void openWorkspaceTerminal(w, false)}>{terminalAction === `${w.id}:offline` ? "Opening…" : "Open Terminal"}</button><button className="network-terminal-button" disabled={state !== "running" || terminalAction !== null || (isLegacy && vm?.state !== "running")} onClick={() => void openWorkspaceTerminal(w, true)}>{terminalAction === `${w.id}:online` ? "Opening…" : "Open with network"}</button></div>
          </div>
        </article>;
      })}</div>}
      <footer className="footer"><span>Local-first by design</span><span>{info ? `${info.name} · ${info.version}` : "Starting controller…"}</span></footer>
    </section>
    {workspaceDialogOpen && <div className="modal-scrim">
      <form className="workspace-dialog" role="dialog" aria-modal="true" aria-labelledby="workspace-dialog-title" onSubmit={(event) => { event.preventDefault(); void createWorkspace(); }}>
        <div className="eyebrow">NEW WORKSPACE</div>
        <h2 id="workspace-dialog-title">Create a workspace VM</h2>
        <p>Orvex creates a separate Linux VM for this workspace and stores its files inside that VM. First creation downloads the Ubuntu image. You can optionally clone a public HTTPS repository.</p>
        <label htmlFor="workspace-name">Workspace name</label>
        <input id="workspace-name" autoFocus maxLength={64} value={workspaceName} onChange={(event) => setWorkspaceName(event.target.value)} placeholder="e.g. api-refactor" />
        <label htmlFor="repository-url">Public HTTPS Git URL <span>(optional)</span></label>
        <input id="repository-url" type="url" value={repoUrl} onChange={(event) => setRepoUrl(event.target.value)} placeholder="https://github.com/org/repository.git" />
        <label className="network-approval"><input type="checkbox" checked={allowVmNetwork} onChange={(event) => setAllowVmNetwork(event.target.checked)} />
          I approve downloading Linux and giving this workspace VM internet access{repoUrl.trim() ? ", including cloning this repository" : ""}.
        </label>
        <div className="workspace-dialog-note needs-vm">This approval enables the VM's network; Orvex does not yet enforce a per-VM firewall. The VM can remain online after creation.</div>
        {workspaceCreating && <div className="vm-progress" role="status" aria-live="polite"><div className="vm-progress-current"><span className="spinner"/><div><strong>{workspaceVmProgress[workspaceSlugPreview(workspaceName)]?.message ?? "Creating the dedicated Linux VM…"}</strong><small>First creation downloads and prepares Ubuntu.</small></div></div></div>}
        {workspaceError && <div className="vm-error" role="alert">{workspaceError}</div>}
        <div className="vm-actions workspace-dialog-actions">
          <button type="button" className="secondary-button" disabled={workspaceCreating} onClick={() => setWorkspaceDialogOpen(false)}>Cancel</button>
          <button type="submit" className="primary-button" disabled={workspaceCreating || !lima?.installed || !workspaceName.trim() || !allowVmNetwork}>
            {workspaceCreating ? <><span className="spinner"/> {repoUrl.trim() ? "Provisioning and cloning…" : "Provisioning VM…"}</> : "Create workspace"}
          </button>
        </div>
      </form>
    </div>}
  </main>;
}
export default App;
