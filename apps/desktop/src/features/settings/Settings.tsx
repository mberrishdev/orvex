import { version } from "../../../package.json";
import { BrandMark } from "../../components/BrandMark";
import { AppUpdates } from "../updates/AppUpdates";
import type { AppInfo } from "../workspaces/types";

export function Settings({ hidden, info, busy }: { hidden: boolean; info: AppInfo | null; busy: boolean }) {
  return <section className="content settings-page" hidden={hidden} aria-label="Settings">
    <header className="topbar" data-tauri-drag-region><span>Orvex <span className="breadcrumb-divider">/</span> Settings</span></header>
    <div className="page-heading"><div><div className="eyebrow">MAKE YOURSELF AT HOME</div><h1>Settings</h1><p>Keep Orvex up to date and choose which releases you receive.</p></div></div>
    <div className="settings-card">
      <div className="settings-about"><BrandMark /><div><h2>Orvex</h2><p>{info ? `Version ${info.version} · ${info.platform === "macos" ? "macOS" : info.platform === "windows" ? "Windows" : info.platform}` : `Version ${version}`}</p></div></div>
      <AppUpdates busy={busy} />
    </div>
    <footer className="footer"><span>Made for a little more focus.</span><span>Your code stays local.</span></footer>
  </section>;
}
