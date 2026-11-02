# Agent session controller contract

**Status:** implementation contract for the first managed-agent milestone  
**Date:** 2026-11-02  
**Provider for the first slice:** Codex app-server in the Linux guest

This contract keeps agent processes behind the Rust controller. The desktop UI calls the controller now; a future `orvex` CLI can call the same operations without driving the UI. Provider-specific protocol details stay inside adapters.

## Boundaries

- An agent starts only after an explicit **Start agent** action. Creating or cloning a workspace never launches it.
- The CLI, its working directory, and repository tools run inside the Linux guest. The host stores Orvex session metadata and sends controller requests; it does not mount project files into the host.
- Codex must be installed and authenticated in the guest. A read-only `codex login status` preflight can check the guest's own login state; it does not validate that a later model request will succeed. Orvex must report a missing CLI or missing login clearly; it must not silently copy host credentials or install software. ([Codex CLI source](https://github.com/openai/codex/blob/main/codex-rs/cli/src/main.rs), accessed 2026-11-02.)
- New workspace VMs receive internet access only after the user approves it during workspace creation, and remain online afterward. Starting a cloud-backed agent still requires an explicit per-session confirmation that it may use that workspace VM's existing connection. This is a consent gate, not a per-process firewall: other guest processes still have network access.
- Codex starts with workspace-scoped writes and request-based approvals. Orvex presents approval requests and sends the user's decision back to the provider. A guest VM and shared VM kernel are not a guarantee against hostile code.

## Controller operations

Operations are provider-neutral. The initial implementation exposes them as Tauri commands; the future CLI should reuse the same Rust controller API.

| Operation | Input | Result |
|---|---|---|
| `agent_status` | workspace ID, provider ID | Installation/auth readiness, active session state, guest/runtime availability, and a user-readable blocker if not ready. It does not install or log in. |
| `agent_start` | workspace ID, provider ID, explicit network grant, optional initial prompt | Stable Orvex session ID and initial state. Rejects if the VM, workspace, provider CLI, or guest authentication is unavailable. |
| `agent_send` | session ID, text | Queues a user message on the existing provider thread. |
| `agent_interrupt` | session ID | Requests cancellation of the active provider turn without deleting the thread. |
| `agent_approval` | session ID, provider request ID, allow/deny choice | Answers a pending provider approval. No approval is inferred from a timeout or UI closure. |
| `agent_stop` | session ID | Stops the provider process and marks the Orvex session stopped; thread metadata remains available for resume if the provider supports it. |
| `agent_list` | optional workspace ID | Lists known session IDs, workspace IDs, provider, state, timestamps, and provider thread ID when available. |

The controller validates every workspace ID against Orvex's workspace store and resolves its guest path itself. It does not accept an arbitrary guest cwd from the frontend or CLI.

## Session state and events

The controller owns a stable Orvex `sessionId`; provider-specific IDs such as a Codex `threadId` are stored as opaque adapter metadata. A session state is one of `starting`, `idle`, `running`, `awaitingApproval`, `stopping`, `stopped`, or `failed`.

Each emitted event uses this envelope:

```json
{
  "sessionId": "orvex-session-id",
  "workspaceId": "workspace-slug",
  "provider": "codex",
  "sequence": 12,
  "timestamp": "RFC-3339 timestamp",
  "kind": "message.delta",
  "data": {}
}
```

Initial event kinds are `session.state`, `message.delta`, `activity.started`, `activity.completed`, `approval.requested`, `approval.resolved`, `diagnostic`, and `session.ended`. Provider adapters may retain the raw provider payload under `data.providerPayload` for debugging, but UI behavior must use normalized fields. Events include a monotonically increasing sequence per session so a reconnecting client can identify gaps.

An `approval.requested` event includes a one-time `requestId`, a typed action (`command`, `fileChange`, or `other`), a readable summary, and the provider's available decisions. The UI shows the proposed action and workspace before enabling approval. Unsupported request types are rejected safely and surfaced as diagnostics rather than auto-approved.

## Process ownership and recovery

For the first Codex proof of concept, the Rust controller starts `codex app-server` in the guest and owns its transport. A small guest-side supervisor is the desired durable process owner once app-window closure and desktop restart recovery are implemented. The UI must not own a provider process directly.

Persist only Orvex session metadata and opaque provider thread IDs, not conversation contents or credentials, unless a later product decision explicitly adds transcript retention. On app reconnect, query controller state and resume the provider thread when supported. If the guest was stopped or the process was lost, show `failed`/`stopped` and offer an explicit resume or new-session action; do not silently replay prompts.

## First implementation acceptance criteria

1. The app reports whether the Linux VM is running and whether Codex is installed and `codex login status` reports a login in the guest, without changing guest state. This is a local preflight, not proof that network requests or the account will work.
2. The user explicitly grants the current VM network access and starts a Codex session in a selected workspace.
3. The agent's working directory is validated as that workspace; the configured sandbox and approval policy are visible before launch.
4. The app streams messages and activity, accepts follow-up input, interrupts a turn, and surfaces command/file approval requests for an explicit user decision.
5. Stopping, crashing, or closing the UI produces an honest state; no prompt or approval is replayed or silently accepted.
6. A future controller CLI can call the same Rust operations without depending on React or Tauri window state.

## Deferred from this contract

- Automatic provider CLI installation, guest login UI, or host token forwarding.
- Cursor/Copilot adapters, MCP configuration, persistent transcript storage, and provider-specific UI extensions.
- Strong network isolation for one agent process while other VM processes remain online. The current VM network boundary is global.
- Windows/WSL2 implementation and VM-per-workspace isolation.
