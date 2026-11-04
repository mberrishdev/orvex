import { useCallback, useEffect, useRef, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export function AppUpdates({ busy }: { busy: boolean }) {
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
      const next = await check({ timeout: 20000 });
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
    <div className="app-update-row">
      <span role="status">{phase === "downloading" ? `Downloading ${downloaded}${progress.total ? ` / ${(progress.total / 1048576).toFixed(1)} MB` : ""}` : phase === "installing" ? "Verifying and installing…" : message || "App updates"}</span>
      {phase === "ready" ? <button className="secondary-button" disabled={busy} onClick={() => void restart()}>Restart Orvex</button> : update ? <button className="secondary-button" disabled={working || busy} onClick={() => void install()}>{working ? "Updating…" : "Install update"}</button> : <button className="secondary-button" disabled={working} onClick={() => void checkForUpdates(true)}>{phase === "checking" ? "Checking…" : "Check for updates"}</button>}
    </div>
    {phase === "downloading" && <progress aria-label="Update download" max={progress.total || undefined} value={progress.total ? progress.downloaded : undefined} />}
    {update && !working && <small>The update is signed and downloaded from GitHub. Installation may close Orvex; save your work first.</small>}
    {busy && (update || phase === "ready") && <small>Finish the current workspace operation before updating or restarting.</small>}
    {error && <p className="runtime-feedback-error" role="alert">{error}</p>}
  </section>;
}
