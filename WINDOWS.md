# Windows support (WSL 2 beta)

Orvex uses an app-owned Ubuntu 24.04 WSL 2 distribution for each workspace.
The initial release targets Windows x64. Linux guest tools, dependencies, Git
checkouts, command sessions, and Docker Engine run inside that distribution.
This supports Linux development projects; it does not provide Windows-native
build tools or Apple SDKs.

## Setup

Install the current WSL release in an Administrator PowerShell:

```powershell
wsl --install --no-distribution
wsl --update
```

Restart Windows if requested. Open Orvex and choose **Check again**. Daily
workspace operations run as your normal Windows account. Orvex does not elevate
itself or install optional Windows features automatically.

To build Orvex from source, install Node.js 22+, pnpm 10+, Rust stable (MSVC),
Microsoft C++ Build Tools with the Desktop development with C++ workload, and
WebView2, following the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/#windows).
Then run `pnpm install` and `pnpm dev` from PowerShell at the repository root.
`pnpm install:cli` builds `orvex.exe`; use the absolute installed path in MCP
client configuration with the argument `mcp`.

## Runtime behavior

- Creation requires the existing internet approval. It downloads a pinned
  Ubuntu Base image from Canonical, checks its SHA-256 digest, imports it with
  `wsl --import --version 2`, and installs the guest tools. First creation may
  take several minutes. Failed creation attempts clean up their owned storage.
- Workspaces run as the Linux user `orvex`, with passwordless sudo for guest
  setup, Docker, and network namespaces. Linux source stays on the distribution's
  own disk. Host project imports stream a Git bundle rather than mount a host
  checkout. Provider sign-in happens inside the workspace.
- Open terminal starts a separate Windows console running the workspace's WSL
  shell. Offline terminal/CLI/MCP execution uses the existing Linux network
  namespace policy; approving network execution uses the distribution's network.
- Stop terminates only that workspace's distribution. Resume starts it again.
  Delete unregisters only an Orvex-owned distribution and removes its disk and
  local ownership record. A matching name without an ownership record is refused.
- Orvex does not edit `.wslconfig`, set a default distribution, shut down all
  WSL distributions, or modify existing personal distributions. Its own
  `wsl.conf` enables systemd and disables Windows drive automount and executable
  interop. Resource and networking settings remain governed by the user's WSL
  configuration.

WSL distributions share a kernel and some networking infrastructure. They are
for trusted development, not a hostile-code containment boundary. Disabling
interop and automount does not change that security model. Services in different
workspaces can contend for ports; there is no per-workspace VM resource cap or
firewall. Keep this distinction in mind when running project setup commands or
agents with guest sudo access.

## Validation

`Desktop checks` runs frontend builds, Rust tests, builds of both binaries, and
MCP protocol checks on Windows and macOS. Hosted CI does not exercise a live
WSL distribution. Before shipping the beta, use a Windows x64 machine with WSL 2
to check:

1. Missing WSL shows setup instructions; installing WSL and choosing Check again
   enables creation. Reboot-required and unsupported-policy errors stay visible.
2. Create two approved workspaces, including one with a public Git repository.
   Confirm different distributions and Linux checkouts, correct branch and user,
   and provider installation/sign-in. Import a committed local Git project via MCP.
3. Open offline and online terminals. Offline `ip route show default` is empty;
   online guest networking works. Check arguments containing spaces and quotes.
4. Run `pnpm check:mcp --workspace <running-workspace-id>` to verify guest cwd,
   bounded output, timeout, persistent sessions, logs, and cancellation.
5. Stop/resume one workspace, restart Orvex, and delete it. The other workspace
   and any pre-existing personal WSL distribution must remain usable. Reject
   deletion for a distribution without an Orvex ownership record.
6. Interrupt creation/download/provisioning and verify cleanup. Test checksum
   failure, unavailable network, Docker/Compose, and concurrent port conflicts.

No live Windows/WSL or Windows terminal validation has been performed on the
macOS development host.

## References

- [Microsoft WSL commands](https://learn.microsoft.com/en-us/windows/wsl/basic-commands)
- [Per-distribution configuration](https://learn.microsoft.com/en-us/windows/wsl/wsl-config)
- [Microsoft WSL security model](https://wsl.dev/technical-documentation/security/)
- [Canonical Ubuntu Base image digests](https://cdimage.ubuntu.com/ubuntu-base/releases/24.04/release/SHA256SUMS)
