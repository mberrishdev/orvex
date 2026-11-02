import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

type AppInfo = { name: string; version: string; platform: string };
type Workspace = { id: string; name: string; branch: string; baseBranch: string | null; status: string; path: string; repoUrl: string | null; vmName: string | null; agentProviders: string[]; composeFile: string | null };
type LimaStatus = { installed: boolean; version: string | null; installSupported: boolean; installer: string | null };
type VmStatus = { state: "notCreated" | "running" | "stopped" | string; instanceName: string; imageBytes: number | null };
type VmProgress = { phase: string; message: string; downloadedBytes?: number | null; workspaceId?: string };
type AgentProvider = { id: string; name: string; executable: string; loginHint: string };
type AgentInstallResult = { providerId: string; providerName: string; installed: boolean; version: string | null; message: string; loginHint: string };
type AgentPreflight = { workspaceId: string; providerId: string; providerName: string; installed: boolean; version: string | null; message: string; loginHint: string };
type WorkspaceCreationResult = { workspace: Workspace; agentInstallations: AgentInstallResult[]; setupCommands: string[]; setupError: string | null };
type ProjectSetupResult = { commands: string[]; completed: number; failedCommand: string | null; message: string };
type WorkspaceServices = {
  workspaceId: string;
  composeFile: string | null;
  dockerInstalled: boolean;
  vmIp: string | null;
  output: string;
  containers: { service: string; state: string; health: string }[];
  ports: { service: string; targetPort: number; publishedPort: number; protocol: string; url: string | null }[];
};

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
  const [workspaceDeleting, setWorkspaceDeleting] = useState<string | null>(null);
  const [pendingWorkspaceDelete, setPendingWorkspaceDelete] = useState<Workspace | null>(null);
  const deleteDialogRef = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = deleteDialogRef.current;
    if (!dialog) return;
    if (pendingWorkspaceDelete && !dialog.open) dialog.showModal();
    else if (!pendingWorkspaceDelete && dialog.open) dialog.close();
  }, [pendingWorkspaceDelete]);
  const [workspaceVmError, setWorkspaceVmError] = useState<string | null>(null);
  const [agentProviders, setAgentProviders] = useState<AgentProvider[]>([]);
  const [agentProviderError, setAgentProviderError] = useState<string | null>(null);
  const [workspaceAgentInstallations, setWorkspaceAgentInstallations] = useState<Record<string, AgentInstallResult[]>>({});
  const [workspaceAgentPreflights, setWorkspaceAgentPreflights] = useState<Record<string, AgentPreflight>>({});
  const [agentPreflightAction, setAgentPreflightAction] = useState<string | null>(null);
  const [workspaceSetupResults, setWorkspaceSetupResults] = useState<Record<string, ProjectSetupResult>>({});
  const [workspaceSetupErrors, setWorkspaceSetupErrors] = useState<Record<string, string>>({});
  const [workspaceServices, setWorkspaceServices] = useState<Record<string, WorkspaceServices>>({});
  const [workspaceServiceAction, setWorkspaceServiceAction] = useState<string | null>(null);
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
  const [baseBranch, setBaseBranch] = useState("");
  const [allowVmNetwork, setAllowVmNetwork] = useState(false);
  const [selectedAgents, setSelectedAgents] = useState<string[]>([]);
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
        const runningWorkspaceIds = new Set(statuses.filter((entry): entry is readonly [string, VmStatus] => entry[1]?.state === "running").map(([id]) => id));
        const discovered = await Promise.all(items.filter((workspace) => workspace.vmName && runningWorkspaceIds.has(workspace.id)).map(async (workspace) => {
          try {
            return await invoke<Workspace>("refresh_workspace_compose_file", { workspaceId: workspace.id });
          } catch {
            return null;
          }
        }));
        const byId = new Map(discovered.filter((workspace): workspace is Workspace => workspace !== null).map((workspace) => [workspace.id, workspace]));
        if (byId.size > 0) setWorkspaces((current) => current.map((workspace) => byId.get(workspace.id) ?? workspace));
        const serviceStates = await Promise.all([...byId.values()].filter((workspace) => workspace.composeFile).map(async (workspace) => {
          try {
            return [workspace.id, await invoke<WorkspaceServices>("manage_workspace_services", {
              workspaceId: workspace.id,
              action: "status",
              allowNetwork: false,
            })] as const;
          } catch {
            return null;
          }
        }));
        setWorkspaceServices(Object.fromEntries(serviceStates.filter((entry): entry is readonly [string, WorkspaceServices] => entry !== null)));
      })
      .catch((reason: unknown) => setError(String(reason)));
  }, []);

  useEffect(() => {
    invoke<AgentProvider[]>("list_agent_providers")
      .then(setAgentProviders)
      .catch((reason: unknown) => setAgentProviderError(String(reason)));
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
    setBaseBranch("");
    setAllowVmNetwork(false);
    setSelectedAgents([]);
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
      const result = await invoke<WorkspaceCreationResult>("create_workspace", {
        name: workspaceName,
        repoUrl: repoUrl.trim() || null,
        baseBranch: baseBranch.trim() || null,
        allowNetwork: allowVmNetwork,
        agentProviderIds: selectedAgents,
      });
      const { workspace } = result;
      setWorkspaces((current) => [workspace, ...current]);
      setWorkspaceAgentInstallations((current) => ({ ...current, [workspace.id]: result.agentInstallations }));
      if (result.setupError) setWorkspaceSetupErrors((current) => ({ ...current, [workspace.id]: result.setupError as string }));
      const workspaceVm: VmStatus = { state: "running", instanceName: workspace.vmName ?? "", imageBytes: null };
      setWorkspaceVms((current) => ({ ...current, [workspace.id]: workspaceVm }));
      if (workspace.composeFile) {
        try {
          const serviceState = await invoke<WorkspaceServices>("manage_workspace_services", {
            workspaceId: workspace.id,
            action: "status",
            allowNetwork: false,
          });
          setWorkspaceServices((current) => ({ ...current, [workspace.id]: serviceState }));
        } catch {
          // The workspace can still be used if service discovery is unavailable.
        }
      }
      setWorkspaceDialogOpen(false);
      setWorkspaceName("");
      setRepoUrl("");
      setBaseBranch("");
      setAllowVmNetwork(false);
      setSelectedAgents([]);
      setWorkspaceVmProgress((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
      if (result.setupCommands.length > 0) {
        const approved = window.confirm(
          `This repository declares setup commands in .orvex/workspace.json:\n\n${result.setupCommands.map((command, index) => `${index + 1}. ${command}`).join("\n")}\n\nRun these commands inside the workspace VM?`,
        );
        if (approved) {
          setWorkspaceSetupResults((current) => ({
            ...current,
            [workspace.id]: { commands: result.setupCommands, completed: 0, failedCommand: null, message: "Running project setup commands…" },
          }));
          try {
            const setupResult = await invoke<ProjectSetupResult>("run_workspace_setup", {
              workspaceId: workspace.id,
              approvedCommands: result.setupCommands,
            });
            setWorkspaceSetupResults((current) => ({ ...current, [workspace.id]: setupResult }));
          } catch (reason) {
            setWorkspaceSetupErrors((current) => ({ ...current, [workspace.id]: String(reason) }));
          }
        } else {
          setWorkspaceSetupResults((current) => ({
            ...current,
            [workspace.id]: { commands: result.setupCommands, completed: 0, failedCommand: null, message: "Setup skipped. Run these commands manually from the workspace terminal." },
          }));
        }
      }
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
      if (action === "start") {
        try {
          const updatedWorkspace = await invoke<Workspace>("refresh_workspace_compose_file", { workspaceId: workspace.id });
          setWorkspaces((current) => current.map((item) => item.id === workspace.id ? updatedWorkspace : item));
          if (updatedWorkspace.composeFile) {
            const services = await invoke<WorkspaceServices>("manage_workspace_services", {
              workspaceId: workspace.id,
              action: "status",
              allowNetwork: false,
            });
            setWorkspaceServices((current) => ({ ...current, [workspace.id]: services }));
          }
        } catch {
          // Keep the workspace VM available even if service status can't be refreshed.
        }
      }
    } catch (reason) {
      setWorkspaceVmError(`${workspace.name}: ${String(reason)}`);
    } finally {
      setWorkspaceVmAction(null);
    }
  }

  async function deleteWorkspace(workspace: Workspace) {
    const isLegacy = !workspace.vmName;
    if (workspaceDeleting !== null) return;
    setWorkspaceDeleting(workspace.id);
    setWorkspaceVmError(null);
    try {
      await invoke("delete_workspace", { workspaceId: workspace.id });
      setPendingWorkspaceDelete(null);
      setWorkspaces((current) => current.filter((item) => item.id !== workspace.id));
      setWorkspaceVms((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
      setWorkspaceVmProgress((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
      setWorkspaceServices((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
      setWorkspaceAgentInstallations((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
      setWorkspaceAgentPreflights((current) => Object.fromEntries(
        Object.entries(current).filter(([key]) => !key.startsWith(`${workspace.id}:`)),
      ));
      setWorkspaceSetupResults((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
      setWorkspaceSetupErrors((current) => { const next = { ...current }; delete next[workspace.id]; return next; });
      if (isLegacy) {
        setVm({ state: "notCreated", instanceName: "orvex-poc", imageBytes: null });
        setVmProgress(null);
        setVmOutput(null);
        setVmError(null);
      }
    } catch (reason) {
      setWorkspaceVmError(`${workspace.name}: ${String(reason)}`);
    } finally {
      setWorkspaceDeleting(null);
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

  async function runWorkspaceServices(workspace: Workspace, action: "start" | "stop" | "status" | "logs") {
    if (action === "start" && !window.confirm(
      `Install Docker in “${workspace.name}” if needed, then start ${workspace.composeFile ?? "the Compose services"}?\n\nThis may download images and run project containers with the VM's current internet access. The Docker daemon has elevated access inside this workspace VM. Services are reachable from this Mac at the workspace VM address.`,
    )) return;
    const key = `${workspace.id}:${action}`;
    setWorkspaceServiceAction(key);
    try {
      const result = await invoke<WorkspaceServices>("manage_workspace_services", {
        workspaceId: workspace.id,
        action,
        allowNetwork: action === "start",
      });
      setWorkspaceServices((current) => ({ ...current, [workspace.id]: result }));
    } catch (reason) {
      setWorkspaceServices((current) => ({
        ...current,
        [workspace.id]: {
          ...(current[workspace.id] ?? { workspaceId: workspace.id, composeFile: workspace.composeFile, dockerInstalled: false, vmIp: null, containers: [], ports: [] }),
          output: String(reason),
        },
      }));
    } finally {
      setWorkspaceServiceAction(null);
    }
  }

  async function checkWorkspaceAgent(workspace: Workspace, providerId: string) {
    const key = `${workspace.id}:${providerId}`;
    setAgentPreflightAction(key);
    try {
      const result = await invoke<AgentPreflight>("check_workspace_agent", {
        workspaceId: workspace.id,
        providerId,
      });
      setWorkspaceAgentPreflights((current) => ({ ...current, [key]: result }));
    } catch (reason) {
      const provider = agentProviders.find((item) => item.id === providerId);
      setWorkspaceAgentPreflights((current) => ({
        ...current,
        [key]: {
          workspaceId: workspace.id,
          providerId,
          providerName: provider?.name ?? providerId,
          installed: false,
          version: null,
          message: String(reason),
          loginHint: provider?.loginHint ?? "",
        },
      }));
    } finally {
      setAgentPreflightAction(null);
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
  const legacyWorkspaceCount = workspaces.filter((workspace) => !workspace.vmName).length;

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
          <div className="workspace-details"><b>{w.name}</b><p>{w.branch}{w.baseBranch ? ` · from ${w.baseBranch}` : ""}</p><small className="workspace-path">{w.path}</small>{w.repoUrl && <small className="workspace-path">{w.repoUrl}</small>}
            <span className={`runtime-badge ${state === "running" ? "ready" : "needs-setup"}`}>{isLegacy ? "SHARED LEGACY VM" : `WORKSPACE VM · ${state.toUpperCase()}`}</span>
          </div>
          <div className="workspace-controls"><span className="status">{w.status}</span>
            <div className="workspace-terminal-actions">
              {!isLegacy && state !== "running" && <button className="primary-button" disabled={!lima?.installed || vmActionBusy || workspaceVmAction !== null} onClick={() => void runWorkspaceVmAction(w, "start")}>{workspaceVmAction === `${w.id}:start` ? <><span className="spinner"/> Starting…</> : state === "stopped" ? "Resume VM" : "Start VM"}</button>}
              {!isLegacy && state === "running" && <button className="secondary-button" disabled={vmActionBusy || workspaceVmAction !== null} onClick={() => void runWorkspaceVmAction(w, "stop")}>{workspaceVmAction === `${w.id}:stop` ? "Stopping…" : "Stop VM"}</button>}
              {(!isLegacy || legacyWorkspaceCount === 1)
                ? <button className="danger-button" disabled={workspaceDeleting !== null || (isLegacy && vmAction !== null)} onClick={() => { setWorkspaceVmError(null); setPendingWorkspaceDelete(w); }}>{workspaceDeleting === w.id ? "Deleting…" : "Delete workspace"}</button>
                : <button className="danger-button" disabled title="This workspace shares the legacy VM with other workspaces. Delete the shared VM to remove all legacy workspaces together.">Delete workspace</button>}
            </div>
            {isLegacy && legacyWorkspaceCount > 1 && <small className="runtime-feedback-error">This workspace shares a VM with {legacyWorkspaceCount - 1} others. Delete the shared VM to remove legacy workspaces together.</small>}
            {!isLegacy && workspaceVmAction === `${w.id}:start` && <small className="runtime-feedback">{workspaceVmProgress[w.id]?.message ?? "Starting this workspace's VM…"}</small>}
            <div className="workspace-terminal-actions"><button className="secondary-button" disabled={state !== "running" || terminalAction !== null || (isLegacy && vm?.state !== "running")} onClick={() => void openWorkspaceTerminal(w, false)}>{terminalAction === `${w.id}:offline` ? "Opening…" : "Open Terminal"}</button><button className="network-terminal-button" disabled={state !== "running" || terminalAction !== null || (isLegacy && vm?.state !== "running")} onClick={() => void openWorkspaceTerminal(w, true)}>{terminalAction === `${w.id}:online` ? "Opening…" : "Open with network"}</button></div>
          </div>
          {!isLegacy && w.agentProviders.length > 0 && <section className="workspace-agent-readiness" aria-label={`Coding CLI readiness for ${w.name}`}>
            <div className="workspace-agent-readiness-heading">
              <strong>Coding CLI readiness</strong>
              <small>Checks installation and version only. Orvex will not start an agent here.</small>
            </div>
            {w.agentProviders.map((providerId) => {
              const key = `${w.id}:${providerId}`;
              const provider = agentProviders.find((item) => item.id === providerId);
              const result = workspaceAgentPreflights[key];
              return <div className="workspace-agent-readiness-row" key={providerId}>
                <div>
                  <b>{provider?.name ?? providerId}</b>
                  {result && <small className={result.installed ? "workspace-agent-ready" : "workspace-agent-not-ready"}>
                    {result.installed ? `Installed${result.version ? ` · ${result.version}` : ""}` : result.message}
                  </small>}
                  {result && <small>{result.installed ? `${result.loginHint} Authentication is not checked by Orvex.` : "After installation, authenticate from the workspace terminal. Authentication is not checked by Orvex."}</small>}
                </div>
                <button className="secondary-button" disabled={state !== "running" || agentPreflightAction !== null} onClick={() => void checkWorkspaceAgent(w, providerId)}>
                  {agentPreflightAction === key ? "Checking…" : result ? "Check again" : "Check CLI"}
                </button>
              </div>;
            })}
          </section>}
          {!isLegacy && w.composeFile && <section className="workspace-services" aria-label={`Services for ${w.name}`}>
            <div className="workspace-services-heading"><strong>Docker Compose</strong><small>{w.composeFile}</small></div>
            <div className="workspace-services-actions">
              <button className="primary-button" disabled={state !== "running" || workspaceServiceAction !== null} onClick={() => void runWorkspaceServices(w, "start")}>
                {workspaceServiceAction === `${w.id}:start` ? <><span className="spinner"/> Installing / starting…</> : workspaceServices[w.id]?.dockerInstalled ? "Start services" : "Install Docker & start"}
              </button>
              <button className="secondary-button" disabled={state !== "running" || workspaceServiceAction !== null || !workspaceServices[w.id]?.dockerInstalled} onClick={() => void runWorkspaceServices(w, "stop")}>{workspaceServiceAction === `${w.id}:stop` ? "Stopping…" : "Stop services"}</button>
              <button className="secondary-button" disabled={state !== "running" || workspaceServiceAction !== null || !workspaceServices[w.id]?.dockerInstalled} onClick={() => void runWorkspaceServices(w, "status")}>{workspaceServiceAction === `${w.id}:status` ? "Refreshing…" : "Refresh"}</button>
              <button className="secondary-button" disabled={state !== "running" || workspaceServiceAction !== null || !workspaceServices[w.id]?.dockerInstalled} onClick={() => void runWorkspaceServices(w, "logs")}>{workspaceServiceAction === `${w.id}:logs` ? "Loading logs…" : "View logs"}</button>
            </div>
            {workspaceServices[w.id]?.containers.map((container) => <div className="workspace-service-row" key={container.service}>
              <b>{container.service}</b><span>{container.state}{container.health ? ` · ${container.health}` : ""}</span>
            </div>)}
            {workspaceServices[w.id]?.ports.map((port) => <div className="workspace-service-port" key={`${port.service}:${port.targetPort}:${port.publishedPort}`}>
              <span>{port.service} · {port.publishedPort} → {port.targetPort}/{port.protocol}</span>
              {port.url ? <button className="service-port-link" onClick={() => void openUrl(port.url as string)}>Open {port.url}</button> : workspaceServices[w.id]?.vmIp ? <code>{workspaceServices[w.id]?.vmIp}:{port.publishedPort}/{port.protocol}</code> : <small>Guest port {port.publishedPort}/{port.protocol}</small>}
            </div>)}
            {workspaceServices[w.id]?.output && <pre className="workspace-service-output">{workspaceServices[w.id]?.output}</pre>}
            {workspaceServices[w.id]?.vmIp && <small className="workspace-service-hint">VM address: {workspaceServices[w.id]?.vmIp} · reachable from this Mac only</small>}
          </section>}
          {!isLegacy && workspaceAgentInstallations[w.id]?.length > 0 && <div className="workspace-cli-results" aria-label={`CLI installation results for ${w.name}`}>
            {workspaceAgentInstallations[w.id].map((item) => <span className={item.installed ? "workspace-cli-installed" : "workspace-cli-failed"} key={item.providerId} title={item.message}>
              {item.installed ? "✓" : "!"} {item.providerName}{item.version ? ` · ${item.version}` : ""}
            </span>)}
          </div>}
          {workspaceSetupResults[w.id] && <small className={`workspace-setup-result ${workspaceSetupResults[w.id].failedCommand ? "workspace-setup-failed" : ""}`}>
            {workspaceSetupResults[w.id].message}{workspaceSetupResults[w.id].failedCommand ? ` · Failed: ${workspaceSetupResults[w.id].failedCommand}` : ""}
          </small>}
          {workspaceSetupErrors[w.id] && <small className="workspace-setup-result workspace-setup-failed">Setup config: {workspaceSetupErrors[w.id]}</small>}
        </article>;
      })}</div>}
      <footer className="footer"><span>Local-first by design</span><span>{info ? `${info.name} · ${info.version}` : "Starting controller…"}</span></footer>
    </section>
    <dialog ref={deleteDialogRef} className="workspace-dialog workspace-delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-workspace-title" aria-describedby="delete-workspace-description" onCancel={(event) => { event.preventDefault(); if (workspaceDeleting === null) setPendingWorkspaceDelete(null); }}>
      {pendingWorkspaceDelete && <>
        <div className="eyebrow">DELETE WORKSPACE</div>
        <h2 id="delete-workspace-title">Delete “{pendingWorkspaceDelete.name}”?</h2>
        <p id="delete-workspace-description">This permanently deletes {pendingWorkspaceDelete.vmName ? "this workspace’s dedicated Linux VM" : "the legacy shared Linux VM"}, its repository, uncommitted changes, installed tools, sign-in data, containers, volumes, and session logs. This cannot be undone.</p>
        <p>Your host project and remote repository are not deleted.</p>
        {workspaceVmError && <div className="vm-error" role="alert">{workspaceVmError}</div>}
        {workspaceDeleting !== null && <p role="status">Deleting the VM and workspace data…</p>}
        <div className="vm-actions workspace-dialog-actions">
          <button type="button" className="secondary-button" autoFocus disabled={workspaceDeleting !== null} onClick={() => setPendingWorkspaceDelete(null)}>Cancel</button>
          <button type="button" className="danger-button" disabled={workspaceDeleting !== null} onClick={() => void deleteWorkspace(pendingWorkspaceDelete)}>{workspaceDeleting !== null ? "Deleting…" : "Delete permanently"}</button>
        </div>
      </>}
    </dialog>
    {workspaceDialogOpen && <div className="modal-scrim">
      <form className="workspace-dialog" role="dialog" aria-modal="true" aria-labelledby="workspace-dialog-title" onSubmit={(event) => { event.preventDefault(); void createWorkspace(); }}>
        <div className="eyebrow">NEW WORKSPACE</div>
        <h2 id="workspace-dialog-title">Create a workspace VM</h2>
        <p>Orvex creates a separate Linux VM and session branch, clones your project into it, and installs the coding CLIs you select. It can also detect a Compose file for services. Setup commands and service startup require your approval.</p>
        <label htmlFor="workspace-name">Workspace name</label>
        <input id="workspace-name" autoFocus maxLength={64} value={workspaceName} onChange={(event) => setWorkspaceName(event.target.value)} placeholder="e.g. api-refactor" />
        <label htmlFor="repository-url">Public HTTPS Git URL <span>(optional)</span></label>
        <input id="repository-url" type="url" value={repoUrl} onChange={(event) => setRepoUrl(event.target.value)} placeholder="https://github.com/org/repository.git" />
        {repoUrl.trim() && <>
          <label htmlFor="base-branch">Start from branch <span>(optional; remote default if blank)</span></label>
          <input id="base-branch" value={baseBranch} onChange={(event) => setBaseBranch(event.target.value)} placeholder="main" />
        </>}
        <label>Coding CLIs to install <span>(choose one or more)</span></label>
        <div className="agent-install-options">
          {agentProviders.map((provider) => <label key={provider.id} className="agent-install-option">
            <input type="checkbox" checked={selectedAgents.includes(provider.id)} onChange={(event) => setSelectedAgents((current) => event.target.checked ? [...current, provider.id] : current.filter((id) => id !== provider.id))} />
            <span>{provider.name}</span>
          </label>)}
        </div>
        {agentProviderError && <div className="vm-error" role="alert">Could not load coding CLI options: {agentProviderError}</div>}
        <div className="workspace-dialog-note">Selected tools are installed inside this VM only. Their account sign-in remains yours to complete.</div>
        <label className="network-approval"><input type="checkbox" checked={allowVmNetwork} onChange={(event) => setAllowVmNetwork(event.target.checked)} />
          I approve downloading Linux and giving this workspace VM internet access{repoUrl.trim() ? ", including cloning this repository" : ""}.
        </label>
        <div className="workspace-dialog-note needs-vm">This approval enables the VM's network; Orvex does not yet enforce a per-VM firewall. The VM can remain online after creation.</div>
        {workspaceCreating && <div className="vm-progress" role="status" aria-live="polite"><div className="vm-progress-current"><span className="spinner"/><div><strong>{workspaceVmProgress[workspaceSlugPreview(workspaceName)]?.message ?? "Creating the dedicated Linux VM…"}</strong><small>First creation downloads and prepares Ubuntu.</small></div></div></div>}
        {workspaceError && <div className="vm-error" role="alert">{workspaceError}</div>}
        <div className="vm-actions workspace-dialog-actions">
          <button type="button" className="secondary-button" disabled={workspaceCreating} onClick={() => setWorkspaceDialogOpen(false)}>Cancel</button>
          <button type="submit" className="primary-button" disabled={workspaceCreating || !lima?.installed || !workspaceName.trim() || !allowVmNetwork || selectedAgents.length === 0}>
            {workspaceCreating ? <><span className="spinner"/> {repoUrl.trim() ? "Provisioning, cloning & installing…" : "Provisioning & installing…"}</> : "Create workspace"}
          </button>
        </div>
      </form>
    </div>}
  </main>;
}
export default App;
