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

## Desktop updates

The macOS Apple Silicon and Windows x64 builds check the latest GitHub release and can install signed updates in-app. Release builds require the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and, when the key is password-protected, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Generate the key pair with `pnpm --filter @orvex/desktop exec tauri signer generate`; keep the private key only in GitHub Actions secrets and place the generated `.pub` value in `apps/desktop/src-tauri/tauri.conf.json`.

## Current capabilities

On macOS, each newly created workspace gets its own Lima Linux VM, and its terminal and VM lifecycle are managed independently. Creating a workspace downloads/prepares Ubuntu and leaves that VM with internet access after an explicit confirmation. The default terminal starts in a Linux network namespace without a usable route; opening a terminal with the VM's network access requires confirmation.

Workspaces created by the earlier prototype use a shared VM; there is no migration flow, so delete that VM if you want to discard those workspaces and start fresh. Per-VM firewall controls and managed AI-agent sessions are not implemented yet. Windows uses a WSL 2 backend for trusted development, with one app-owned Ubuntu distribution per workspace; see [Windows setup and validation](WINDOWS.md). Live Windows runtime validation is still pending. See the [roadmap](ROADMAP.md) for status and next steps.
