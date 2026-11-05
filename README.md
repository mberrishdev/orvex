# Orvex

Orvex is a local-first desktop app for creating development task workspaces and working in them with coding-agent CLIs.

## Development prerequisites

- Node.js 22+
- pnpm 10+
- Rust stable
- Tauri 2 platform prerequisites: https://v2.tauri.app/start/prerequisites/

## Run

```sh
pnpm install
pnpm dev
```

`pnpm dev` launches the Tauri desktop application. `pnpm build` checks and builds the frontend; `pnpm check:rust` checks the Rust workspace.

## Connect your AI

In **Agent connections**, choose **Connect your AI** and select Codex, Claude Code,
GitHub Copilot CLI, Pi, Cursor, or Gemini CLI. Orvex previews the user configuration
path and adds its local MCP server when you click **Add**. Existing settings and
other servers are preserved; existing files receive a sibling `orvex-backup-*`
copy. Conflicting Orvex entries and malformed configurations require manual review.
Custom client configuration directories are respected for Codex, Copilot, and Pi.

Restart your AI client or reload its MCP servers, then ask it to list your Orvex
workspaces. The app shows a live connection only after an MCP handshake. Pi requires
a version with built-in MCP support; older versions may need an extension. Other
MCP clients can use the copyable JSON configuration (adapt the top-level key to the
client). These connections target clients on the host computer, outside workspace VMs.

The desktop executable also accepts `mcp` to serve over stdio without opening a
window, so no separate CLI installation is needed. Keep the installed app at the
configured path; reconnect from setup if you move it. Development configurations
point to the development build and should be added again from the installed app.

## Desktop updates

Open **Settings → Software updates** to check for, install, and restart into signed updates on macOS Apple Silicon and Windows x64. Choose **Stable** for stable releases or **Beta** for previews and stable releases. The channel is saved on this device; beta builds default to Beta. Switching channels never downgrades the installed version. Update checks continue automatically at startup and every six hours, including while you use Workspaces. Installation and restart wait until current workspace operations finish. Release builds require the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and, when the key is password-protected, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Generate the key pair with `pnpm --filter @orvex/desktop exec tauri signer generate`; keep the private key only in GitHub Actions secrets and place the generated `.pub` value in `apps/desktop/src-tauri/tauri.conf.json`.

## Current capabilities

On macOS, each newly created workspace gets its own Lima Linux VM, and its terminal and VM lifecycle are managed independently. Creating a workspace downloads/prepares Ubuntu and leaves that VM with internet access after an explicit confirmation. The default terminal starts in a Linux network namespace without a usable route; opening a terminal with the VM's network access requires confirmation.

Workspaces created by the earlier prototype use a shared VM; there is no migration flow, so delete that VM if you want to discard those workspaces and start fresh. Per-VM firewall controls and managed AI-agent sessions are not implemented yet. Windows uses a WSL 2 backend for trusted development, with one app-owned Ubuntu distribution per workspace; see [Windows setup and validation](WINDOWS.md). Live Windows runtime validation is still pending. See the [roadmap](ROADMAP.md) for status and next steps.
