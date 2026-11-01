# Orvex

Orvex is a local-first desktop app for creating isolated development task workspaces. The product is in its first scaffold milestone; workspace provisioning is not implemented yet.

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

## Current milestone

The desktop window loads a React dashboard. Rust-backed Tauri commands report app/platform information and an empty workspace list. The “new workspace” actions remain disabled. No VM, container, Git, credential or agent integrations are present yet.

See [implementation plan](output/IMPLEMENTATION_PLAN.md), [architecture recommendation](output/ARCHITECTURE_RECOMMENDATION.md) and the [research dossier](research/README.md).
