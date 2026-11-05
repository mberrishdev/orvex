import { useCallback, useEffect, useRef, useState } from "react";
import { type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { checkForChannelUpdates, type UpdateChannel } from "./checkForUpdates";
import { version } from "../../../package.json";

function savedChannel(): UpdateChannel {
  try {
    const saved = localStorage.getItem("orvex.updateChannel");
    if (saved === "stable" || saved === "beta") return saved;
  } catch { /* Keep updates available when storage is unavailable. */ }
  return version.includes("-") ? "beta" : "stable";
}

export function AppUpdates({ busy }: { busy: boolean }) {
  const [channel, setChannel] = useState<UpdateChannel>(savedChannel);
  const channelRef = useRef(channel);
  const [update, setUpdate] = useState<Update | null>(null);
  const [phase, setPhase] = useState<"idle" | "checking" | "downloading" | "installing" | "ready">("idle");
  const [message, setMessage] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState({ downloaded: 0, total: 0 });
  const inFlight = useRef(false);
  const current = useRef<Update | null>(null);
  const mounted = useRef(false);
  const installed = useRef(false);

  const checkForUpdates = useCallback(async (manual = false) => {
    if (inFlight.current || installed.current) return;
    inFlight.current = true;
    setPhase("checking");
    setError(null);
    try {
      const next = await checkForChannelUpdates(channelRef.current);
      if (!mounted.current) { await next?.close(); return; }
      await current.current?.close();
      current.current = next;
      setUpdate(next);
      setMessage(next ? `Orvex ${next.version} is available.` : manual ? "You’re up to date." : "");
    } catch (reason) {
      if (mounted.current) setError(`Could not check for updates: ${String(reason)}`);
    } finally {
      inFlight.current = false;
      if (mounted.current) setPhase("idle");
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    if (!import.meta.env.DEV) void checkForUpdates();
    const timer = setInterval(() => void checkForUpdates(), 6 * 60 * 60 * 1000);
    return () => {
      mounted.current = false;
      clearInterval(timer);
      if (!inFlight.current) void current.current?.close();
    };
  }, [checkForUpdates]);

  async function changeChannel(next: UpdateChannel) {
    if (inFlight.current || installed.current) return;
    channelRef.current = next;
    setChannel(next);
    try { localStorage.setItem("orvex.updateChannel", next); } catch { /* Use the choice for this session. */ }
    setMessage("");
    setUpdate(null);
    void checkForUpdates(true);
  }

  async function install() {
    if (!update || busy || inFlight.current) return;
    inFlight.current = true;
    setError(null);
    setProgress({ downloaded: 0, total: 0 });
    setPhase("downloading");
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") setProgress({ downloaded: 0, total: event.data.contentLength ?? 0 });
        if (event.event === "Progress") setProgress((value) => ({ ...value, downloaded: value.downloaded + event.data.chunkLength }));
        if (event.event === "Finished") setPhase("installing");
      }, { timeout: 120000 });
      installed.current = true;
      setPhase("ready");
      setMessage("Update installed. Restart Orvex to finish.");
      await update.close();
      current.current = null;
      setUpdate(null);
    } catch (reason) {
      setError(`Could not install the update: ${String(reason)}`);
      setPhase("idle");
    } finally {
      inFlight.current = false;
    }
  }

  async function restart() {
    if (busy || inFlight.current) return;
    inFlight.current = true;
    setError(null);
    try { await relaunch(); }
    catch (reason) { setError(`Could not restart Orvex: ${String(reason)}. Quit and reopen it to finish updating.`); }
    finally { inFlight.current = false; }
  }

  const working = phase === "checking" || phase === "downloading" || phase === "installing";
  const downloaded = `${(progress.downloaded / 1048576).toFixed(1)} MB`;
  return <section className="app-updates" aria-label="App updates">
    <h3>Software updates</h3>
    <p className="update-description">Orvex checks automatically every six hours. You choose when to install.</p>
    <div className="update-channel"><label htmlFor="update-channel">Release channel</label><select id="update-channel" value={channel} disabled={working || phase === "ready"} onChange={(event) => void changeChannel(event.target.value as UpdateChannel)}><option value="stable">Stable</option><option value="beta">Beta</option></select></div>
    <small className="update-description">{channel === "beta" ? "Receive beta previews and stable releases. Beta versions may have rough edges." : "Receive stable releases only. Switching channels never downgrades your installed version."}</small>
    <div className="app-update-row">
      <span role="status">{phase === "downloading" ? `Downloading ${downloaded}${progress.total ? ` / ${(progress.total / 1048576).toFixed(1)} MB` : ""}` : phase === "installing" ? "Verifying and installing…" : message || "App updates"}</span>
      {phase === "ready" ? <button className="secondary-button" disabled={busy} onClick={() => void restart()}>Restart Orvex</button> : update ? <button className="secondary-button" disabled={working || busy} onClick={() => void install()}>{working ? "Updating…" : "Install update"}</button> : <button className="secondary-button" disabled={working} onClick={() => void checkForUpdates(true)}>{phase === "checking" ? "Checking…" : "Check for updates"}</button>}
    </div>
    {phase === "downloading" && <progress aria-label="Update download" max={progress.total || undefined} value={progress.total ? progress.downloaded : undefined} />}
    {update && !working && <small>The update is signed and downloaded from GitHub. Installation may close Orvex; save your work first.</small>}
    {update?.body && <details className="update-notes"><summary>Release notes</summary><p>{update.body}</p></details>}
    {busy && (update || phase === "ready") && <small>Finish the current workspace operation before updating or restarting.</small>}
    {error && <p className="runtime-feedback-error" role="alert">{error}</p>}
  </section>;
}
