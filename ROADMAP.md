# Orvex roadmap

Last updated: 2026-11-02

Orvex is a local-first desktop app for creating development workspaces and letting the user run their chosen coding-agent CLI inside them. This roadmap tracks what exists, what we plan to build next, and what is intentionally deferred.

## Current product boundaries

- New workspaces on macOS get a named Lima Linux VM of their own. The earlier prototype used a shared VM; migration is not supported, so those old workspaces must be discarded to start fresh.
- **Preferred architecture:** one VM per workspace, based on the user's preference. This is implemented for new workspaces; startup time, memory/disk overhead, and management complexity still need runtime validation.
- A cloned repository is ready to open in a terminal; Orvex does not install its dependencies or automatically start project services.
- The default workspace terminal is started in an offline Linux namespace. The explicit online terminal gets the VM's network access after confirmation. The VM itself and other guest processes still have network access.
- Orvex does not currently start or supervise an AI agent process. The user launches a CLI in the terminal.
- Docker and Compose support, Windows runtime support, and project environment setup are not implemented.
- The shared-VM prototype is for trusted code. It is not a strong security boundary between hostile agents or workspaces.

## Completed

- [x] Create the pnpm/Cargo monorepo with a Tauri 2, React, TypeScript, and Rust desktop app.
- [x] Detect Lima and offer an in-app Homebrew installation flow on macOS.
- [x] Start, stop, resume, inspect, and delete the app-owned Lima proof-of-concept VM; show startup progress.
- [x] Create an empty workspace folder in the VM.
- [x] Clone a public HTTPS Git repository after explicit approval for the clone action.
- [x] Add per-workspace Open Terminal and Open with network actions. The online action asks for confirmation; the offline terminal uses nested Linux user and network namespaces.
- [x] Ignore research, output, and generated build artifacts in Git.
- [x] Add the MIT license and push the `main` branch to GitHub.
- [x] Verify the Lima runtime and repository clone flow on the user's Mac.

## Next: dedicated workspace VMs

**Goal:** Give each workspace an independently managed Linux VM. The shared-VM prototype data was disposable and has been deleted from the development Mac.

- [x] Create a named Lima VM when creating a new workspace and place the repository inside that VM.
- [x] Route each workspace terminal to its recorded VM.
- [x] Add per-workspace start, stop, resume, and delete controls; deleting a workspace requires confirmation and deletes its VM disk.
- [x] Keep pre-existing workspaces on the original shared VM rather than moving or deleting their files implicitly.
- [ ] Verify two new workspace VMs can run at once, stop/resume independently, and open terminals in the correct workspace.
- [ ] Measure concurrent VM memory, disk, and startup costs; current default allocation is 2 CPUs, 2 GiB RAM, and 8 GiB disk per VM.

**Completion check:** Two dedicated workspace VMs can run concurrently; stopping or deleting one does not affect the other.

## Then: user-directed agent sessions

**Goal:** Let the user start and manage one chosen coding-agent CLI from Orvex, without starting an agent automatically when a repository is cloned.

- [x] Research agent session interfaces and choose a first integration direction. See [agent session interface research](docs/AGENT_SESSION_INTERFACES.md).
- [x] Define the provider-neutral controller operations, session states, event envelope, approval flow, and process ownership boundary. See [agent session controller contract](docs/AGENT_SESSION_CONTRACT.md).
- [ ] Verify the pinned Codex CLI version and app-server protocol against the required session lifecycle, approvals, and resume behavior.
- [ ] Decide how Codex authentication and model-network access are granted to the guest.
- [ ] Implement a read-only Codex guest preflight, then start the selected CLI only when the user clicks **Start agent** and grants network access.
- [ ] Keep the process alive independently of the UI window; show live output and current state.
- [ ] Support sending input, interrupting/stopping, and reconnecting to an active session.
- [ ] Make agent network access explicit because model-provider requests need network access.
- [ ] Show the workspace path and Git status so the user can confirm where the agent is working.
- [ ] Verify the agent only changes the intended workspace under the current trusted-workspace model; document the shared-VM limitation.

**Completion check:** The user can start one CLI in a chosen workspace, see its output, send a follow-up, disconnect/reconnect, stop it, and inspect the resulting Git changes.

## Then: let the main agent manage Orvex

**Goal:** Give the user's main AI agent a stable way to control workspace sessions without driving the desktop UI.

- [ ] Keep workspace and session operations behind a UI-independent Rust controller interface.
- [ ] Add an `orvex` CLI for listing workspaces, starting/stopping sessions, checking status, sending input, and reading logs.
- [ ] Return stable session IDs and machine-readable output for agent use.
- [ ] Require explicit grants for network access and other consequential operations.
- [ ] Consider an MCP adapter only after the CLI and controller contract are stable.

**Completion check:** The desktop UI and the main agent can operate the same session through the same controller, with the same permissions and session state.

## Later: workspace services and ports

This is separate from automatic project environment setup. A user could explicitly start services when they need them.

- [ ] Choose the supported Docker Engine arrangement inside the Linux runtime.
- [ ] Give each workspace a unique Compose project, network, and persistent-volume namespace.
- [ ] Allocate host ports per workspace and bind forwarded ports to localhost by default.
- [ ] Show service state, logs, and the workspace URL; support starting and stopping services.
- [ ] Run two copies of the same Compose project concurrently and verify there are no port or data collisions.
- [ ] Document that Compose separation in one VM does not isolate hostile code from the shared kernel or daemon.

**Completion check:** Two workspaces can run the same service ports internally (for example, `3000`) while Orvex assigns distinct localhost ports and keeps their service data separate.

## Later: stronger workspace isolation

- [ ] Decide the supported trust levels: trusted parallel work, or hostile/untrusted agent code.
- [ ] Test workspace filesystem, process, network, and Docker-daemon access across two simultaneous workspaces.
- [ ] Finalize the isolation boundary based on runtime measurements and supported trust levels.
- [ ] Make the UI describe the tested boundary and fail closed when a requested policy cannot be enforced.

**Completion check:** Security claims match tested behavior for sibling workspace files, processes, network, credentials, and container control.

## Later: Windows support

- [ ] Select and prototype the Windows Linux-runtime provider.
- [ ] Keep the user's existing WSL distributions and settings untouched by default.
- [ ] Match the core workspace and agent-session workflows, and document platform-specific differences.

## Deferred by current scope

- [ ] Automatic dependency/runtime installation and project setup scripts.
- [ ] Automatic service startup immediately after cloning.
- [ ] Auto-starting an AI agent or generating/sending its task prompt during clone.
- [ ] Private-repository credential management, snapshots/restore, and broad editor integrations.

These remain possible future features, but they are not prerequisites for the current workspace and user-directed-agent workflow.
