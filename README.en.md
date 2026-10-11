<p align="center">
  <img src="EndlessVibe.png" alt="EndlessVibe logo" width="156">
</p>

<p align="center"><a href="README.md">简体中文</a> · <strong>English</strong></p>

<h1 align="center">EndlessVibe</h1>

<p align="center">
  <strong>Connect your local development environment to AI assistants, on your terms.</strong>
  <br>
  A self-hosted Rust MCP server for authorized project access, source editing, command execution, Git workflows, and connected development nodes.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.3.5-2563eb" alt="Version 0.3.5">
  <img src="https://img.shields.io/badge/Rust-1.88%2B-de6a36?logo=rust&logoColor=white" alt="Rust 1.88+">
  <img src="https://img.shields.io/badge/platform-Linux-34495e?logo=linux&logoColor=white" alt="Linux">
  <img src="https://img.shields.io/badge/MCP-Streamable%20HTTP-7c3aed" alt="MCP Streamable HTTP">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-22a06b" alt="MIT License"></a>
</p>

<p align="center">
  <a href="#screenshots">Screenshots</a> ·
  <a href="#features">Features</a> ·
  <a href="#quick-start">Quick Start</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="#security">Security</a> ·
  <a href="#documentation">Documentation</a>
</p>

---

## Overview

**EndlessVibe** connects ChatGPT and other Model Context Protocol (MCP) clients to explicitly authorized local code projects. It includes a browser-based local dashboard for project access, command jobs, audit trails, and node administration.

EndlessVibe is **not** a cloud IDE or a multi-tenant remote shell. It is designed for one owner running their own server, with explicit permissions, per-project coordination, reviewable changes, and recoverable long-running work.

- **MCP Server** — Streamable HTTP transport with OAuth 2.0 authorization.
- **Local Dashboard** — Inspect projects, tasks, activity, operations, configuration, and nodes.
- **Project Runtime** — File APIs, command execution, Git, task checkpoints, and durable jobs.
- **Node Transfer (optional)** — Discover and pair nearby EndlessVibe nodes, then work on explicitly granted remote projects.

## Screenshots

### Dashboard

![EndlessVibe Dashboard](docs/screenshots/dashboard-en-light.jpg)

<sub>Light theme · English interface — service status, execution metrics, and the local management dashboard.</sub>

| Projects | Tasks |
| :---: | :---: |
| ![Project management](docs/screenshots/projects-en-light.jpg) | ![Task management](docs/screenshots/tasks-en-light.jpg) |
| Workspace / Project access and configuration | Jobs, stages, and checkpoints |

| Operations | Nodes |
| :---: | :---: |
| ![Operation history](docs/screenshots/operations-en-light.jpg) | ![Node management](docs/screenshots/nodes-en-light.jpg) |
| Auditable operations and execution history | Node discovery, pairing, and request recovery |

<sub>Screenshots show the latest worktree frontend with live local Dashboard data, in Light / English mode. Personal machine identifiers have been redacted. Node Transfer is disabled by default and requires explicit configuration. The Chinese README shows Dark / 简体中文.</sub>

## Features

| Area | Capabilities |
| --- | --- |
| **Workspaces & Projects** | Two-level authorization model, per-project file/command/Git permissions, independent operation locks |
| **Files & Search** | Secure directory listing, read/write/create, exact patching, code search, SHA-256 conflict detection |
| **Commands & Jobs** | Asynchronous commands, durable job IDs, paged output, cancellation, timeout, request deduplication |
| **Git Workflow** | Reviewable status/diff/log/commit, optional local Git mutation, separately authorized push |
| **Tasks & Recovery** | `task_id + stage` checkpoints, job association, committed recovery points, restart diagnostics |
| **Dashboard** | Runtime charts, Projects, Tasks, Operations, Nodes, configuration, and storage management |
| **Authentication** | OAuth 2.0 Authorization Code, PKCE, dynamic client registration, tool scopes |
| **Node Transfer** | LAN discovery, TLS pairing, scoped grants, persistent remote request status and history |
| **Execution Profiles** | Bubblewrap by default, with explicitly configured Development / Trusted Host profiles |
| **Docker (optional)** | Restrained Docker Engine operations scoped to explicitly allowed containers |

### Workspace and Project Model

A `Workspace` establishes the filesystem management boundary; a `Project` is the unit of permissions and operations:

```text
Workspace: projects                  /home/user/projects
├── Project: website                 website/
├── Project: kernel                  kernel/
└── Project: tools                   tools/
```

Different Projects can run independent operations concurrently. File changes, commands, and Git mutations on the **same** Project share an operation lock to reduce conflicting changes.

## Quick Start

### Requirements

- **Linux 5.6+, macOS, or Windows 10/11** (macOS and Windows are newly adapted targets; native builds and tests are required before calling them release-ready)
- Rust / Cargo **1.88+**
- Git and a C/C++ compiler toolchain installed on the target operating system
- Linux requires Bubblewrap and unprivileged user namespaces by default; on macOS and Windows, command execution is disabled by default until an administrator explicitly acknowledges the unsandboxed host backend

Linux refuses to run as root. Capability-based filesystem access applies to all supported builds; **Bubblewrap is Linux-only, and the Docker Unix Socket transport is not implemented on Windows**. See [Cross-Platform Support and Limitations](docs/PORTABILITY.md).

### 1. Build

From the repository root:

```bash
cargo build --release
cargo test --locked
```

Optional on Linux only: verify that Bubblewrap can access the configured toolchains:

```bash
./target/release/endlessvibe --check-sandbox
```

### 2. Initialize a Workspace

Create an existing directory as the Workspace root and initialize the service:

```bash
mkdir -p "$HOME/projects"

./target/release/endlessvibe \
  --init \
  --workspace "projects=$HOME/projects" \
  --public-url https://mcp.example.com
```

`--init` creates the configuration and private state, including a random owner key. A Workspace does **not** automatically grant file, command, or Git access to its children.

### 3. Register a Project

```bash
# The target directory must already exist.
./target/release/endlessvibe \
  --add-project "projects:website=$HOME/projects/website"
```

Projects must be located inside an authorized Workspace root. Use `--read-only` or `--no-exec` to reduce the permissions of a newly registered Project.

### 4. Start the Server

```bash
./target/release/endlessvibe
```

| Service | Default endpoint |
| --- | --- |
| **Dashboard** | [http://127.0.0.1:20001](http://127.0.0.1:20001) (loopback only) |
| **MCP** | `/mcp` on port `20000` by default; use HTTPS for public access |
| **Node Transfer** | Disabled by default; requires configuration and approval on both nodes |

Use `--status`, `--stop`, and `--restart` for process management. On Windows, commands verify the process creation time and signal a per-instance graceful shutdown event. When upgrading from an older Windows build, stop it once with Ctrl+C or the service manager, then launch the new binary before using `--restart`. This does not require the `host` execution backend.

### 5. Connect an MCP Client

In ChatGPT or another MCP-compatible client:

```text
Name:           EndlessVibe
Server URL:     https://mcp.example.com/mcp
Authentication: OAuth 2.0
```

Replace the example origin with your configured `server.public_url`. Your reverse proxy or tunnel must also route the OAuth endpoints, not just `/mcp`. **Never place an `owner.key` or access token in URLs.**

See [OAuth](docs/OAUTH.md) and [Cloudflare Tunnel](docs/DOCKER_TUNNEL.md).

## LAN-only mode (no public ChatGPT URL)

Use **`--lan-only`** to connect EndlessVibe instances via **TLS 1.3 Transfer pairing** without exposing MCP or OAuth over plaintext LAN HTTP.

**New Windows installation** (the directory must already exist):

```powershell
.\target\debug\endlessvibe.exe --init --lan-only --workspace "projects=C:\path\to\projects"
```

**Existing Windows installation** (no need to delete the config or owner key):

```powershell
# Stop the running EndlessVibe instance, then convert the config once
.\target\debug\endlessvibe.exe --lan-only

# Start the installed copy, not Cargo's build output
.\deploy\windows\EndlessVibe.ps1 -Action Update
```

**Windows `cargo build` fails with `Access is denied (os error 5)`:** Windows cannot replace a running `target\debug\endlessvibe.exe`. Restarting Windows and launching the same executable simply locks it again. From the project root in Windows PowerShell, run this once:

```powershell
.\deploy\windows\EndlessVibe.ps1 -Action Update
```

The script builds into `target\windows-stage`, installs an immutable executable copy under `%LOCALAPPDATA%\Programs\EndlessVibe\releases\<SHA256>\endlessvibe.exe`, gracefully stops the previous service using its verified `--stop` control, then starts the installed copy. It never force-kills a process or replaces config, owner keys, or Project grants. The manifest is now published atomically using a non-empty backup path *before* stopping the service; if the new instance cannot start, the script attempts to restore both the old manifest and service. Afterwards, an ordinary `cargo build` can overwrite its default `target\debug` output without colliding with the running program.

```powershell
.\deploy\windows\EndlessVibe.ps1 -Action Update   # Build, install, restart
.\deploy\windows\EndlessVibe.ps1 -Action Start    # Start installed version without building
.\deploy\windows\EndlessVibe.ps1 -Action Status   # Report state and installed path
.\deploy\windows\EndlessVibe.ps1 -Action Stop     # Graceful stop
```

For custom configuration, pass an absolute `-ConfigPath 'C:\path\to\config.toml'` on the initial update; subsequent starts reuse it. **Update any Task Scheduler/NSSM/service startup entry to invoke this script with `-Action Start`**, instead of launching the Cargo target executable. If the previous binary does not support `--stop`, or it runs under another logon session, stop it through its existing supervisor first. The script never force-kills an unrelated process.

The setting persists in `config.toml`, preserving all Workspace/Project grants and the owner key. Tokens issued for the previous public OAuth resource become invalid.

**Windows AppContainer self-check:** After stopping the Windows service, run `cargo build --locked --target-dir target/windows-stage` and `.\target\windows-stage\debug\endlessvibe.exe --probe-windows-sandbox`. This verifies the native lowbox token, Job Object and fixed `hash_input` receipt, with a combined 64 KiB private stdout/stderr limit, tamper detection and Job termination on overflow; it does not enable a general command sandbox or read user Projects. See [Windows sandbox limitations](docs/PORTABILITY.md).

The sandbox self-check now stages only the already-running EndlessVibe EXE (up to 128 MiB), copies through a pinned file handle, verifies SHA-256 before and after the lowbox worker runs, and reports success only after profile and recovery journal cleanup. It does not accept Project binaries or third-party executables. A bounded PE64 static-import audit rejects unknown DLLs and delay-load imports; MSVC runtimes are checked in Windows System32 and rehashed after execution. This is not complete Windows loader-path attestation. The fixed Worker now pauses at two private gates so its parent can inspect mapped DLL paths via Toolhelp before and after the operation; modules outside System32 are rejected. These snapshots cannot detect transient loads between checks or prove Authenticode identity.

Worker SystemRoot, SystemDrive and PATH now come from native Windows system-directory APIs instead of inherited host variables. ComSpec is absent and extra variables are limited to the ENDLESSVIBE_ namespace.

The broker now compares each mapped section's kernel-reported NT-device filename against its Toolhelp path through GetMappedFileNameW and QueryDosDeviceW at both barriers; this is not a signature or loaded-file-byte identity proof.

It also pins each reopened image file by Windows handle and compares volume serial, file index, size and last-write time across both checks, rejecting same-name replacements. This does not prove mapped-memory bytes or Authenticode signatures.

- Local MCP (OAuth): `http://127.0.0.1:20000/mcp`, **loopback only**, not an HTTP endpoint for other LAN computers.
- Local Dashboard: `http://127.0.0.1:20001/nodes`, **loopback only**.
- Node Transfer: **TCP 20002 / TLS 1.3**; discovery: **UDP 20003 / multicast**. Open these only on a trusted LAN.
- The **parent** initiates pairing; both devices compare the six-digit verification code, and **only the child clicks Accept or Reject**. The parent finishes automatically after the child's decision. Pairing grants **no Projects by default**; discovery alone is not authorization.
- Later, register a Project from the Windows **Projects** page (hot reload is supported) and explicitly grant it under **Nodes → Trusted Nodes → Manage Project grants**. **New Projects never become accessible automatically**; updated grants take effect immediately, without re-pairing.

Do not combine `--lan-only` with `--public-url` or `--bind`. Direct MCP HTTP access from another computer still requires an HTTPS-protected endpoint; **never transmit OAuth bearer tokens over plaintext LAN HTTP**.

See [LAN Transfer pairing](docs/TRANSFER.md).

## Usage

### Read and edit a file

```text
list_workspaces
  → list_projects(workspace)
  → read_file(workspace, project, path)
  → apply_patch(workspace, project, path, expected_sha256, edits)
```

Existing files require the returned `expected_sha256` for conflict-safe edits. Use the literal `MISSING` when creating a new file. Patches use exact text replacements rather than unified diff input.

### Run a command job

```json
{
  "workspace": "projects",
  "project": "website",
  "program": "cargo",
  "args": ["test", "--locked"],
  "request_id": "website-test-001",
  "timeout_seconds": 120
}
```

A command returns a Job ID. Query it through `get_job` / `get_job_output`. After connection loss, inspect the original request before taking further action; **do not blindly retry a mutation under a different request ID**.

### Review and commit changes

```text
git_status
  → git_diff(paths)
  → Review changes
  → git_commit(paths, expected_head, expected_diff_sha256)
```

`git_commit` includes only reviewed paths and does not push by default. Remote pushes require an additional per-Project permission.

## Architecture

```mermaid
flowchart TB
    Client["ChatGPT / MCP Clients"] -->|"OAuth 2.0 + PKCE"| MCP["Rust MCP Server"]
    Browser["Local Browser"] --> Dashboard["Dashboard · 127.0.0.1:20001"]
    MCP --> Runtime["Workspace / Project Runtime"]
    Dashboard --> Runtime
    Runtime --> Files["Files & Search"]
    Runtime --> Jobs["Command Jobs & Checkpoints"]
    Runtime --> Git["Git Review & Commit"]
    Runtime --> Store["SQLite State & Audit"]
    Runtime -.->|"Optional TLS pairing"| Nodes["Remote EndlessVibe Nodes"]
```

- **Backend:** Rust with Axum, Tokio, rmcp, Rusqlite, and Rustls.
- **Frontend:** Local static assets bundled with the Rust binary; no separate frontend server.
- **Storage:** SQLite persists authorization, job/task metadata, operation logs, and node records.
- **Isolation:** Project-scoped locks and authorization, secure paths, and configurable command execution backends.

## Configuration

Configuration defaults to `~/.config/endlessvibe/config.toml` and state to `~/.local/state/endlessvibe`, respecting the corresponding XDG environment variables.

Example Workspace / Project configuration:

```toml
[[workspaces]]
id = "projects"
path = "/home/user/projects"

[[workspaces.projects]]
id = "website"
path = "website"
allow_write = true
allow_exec = true
allow_git_commit = true
allow_git_mutation = false
allow_git_push = false
execution_profile = "isolated"
```

Execution profiles:

| Profile | Purpose |
| --- | --- |
| `isolated` | Default restricted environment using Bubblewrap |
| `development` | For trusted development projects; host networking may be enabled by configuration |
| `trusted_host` | **High-risk opt-in:** with an acknowledged Host backend, run locally installed executables on PATH |

`trusted_host` is **not enabled by default**. Switching the **global** execution backend to Host removes Bubblewrap isolation for other command-enabled Projects too. Use it only in environments you control.

See [Example Configuration](config/config.example.toml) and [Execution](docs/EXECUTION.md).

## Security

EndlessVibe is designed for **single-owner, self-hosted** workflows, not for exposing your host to untrusted users.

- **Explicit access control:** Private MCP tools require OAuth and appropriate scopes; Project permissions are configured separately.
- **Path safety:** File APIs reject traversal outside a Project root and symlink-based boundary bypasses.
- **Sandbox by default:** Bubblewrap does not mount your full HOME, SSH credentials, Docker socket, or the service state directory by default.
- **Privileged actions are opt-in:** Shell, Docker changes, Git pushes, and Host execution have separate configuration requirements.
- **Recovery and auditability:** Persisted jobs, operation records, and checkpoints support diagnosis. Uncertain execution outcomes are never assumed not to have happened.
- **Loopback-only Dashboard:** Do not expose the management UI through a public reverse proxy.

Review [Security](docs/SECURITY.md) and [Execution](docs/EXECUTION.md) before enabling public MCP access or switching execution backends.

## Development

```bash
cargo fmt --check
cargo test --locked
cargo build --release
```

Keep changes focused, review diffs, and run relevant tests before proposing a commit. Issues and pull requests are welcome under the MIT License.

## Documentation

| Document | What it covers |
| --- | --- |
| [MCP Tools](docs/TOOLS.md) | Exposed tools, arguments, and behavior |
| [Execution](docs/EXECUTION.md) | Bubblewrap, Host, toolchains, jobs, preflight |
| [Platform Support](docs/PORTABILITY.md) | Portability status and OS-specific limitations |
| [Security](docs/SECURITY.md) | Authorization, trust boundaries, deployment |
| [OAuth](docs/OAUTH.md) | OAuth 2.0, PKCE, client integration |
| [Git Recovery](docs/GIT_RECOVERY.md) | Review tokens, Git commit, recovery |
| [Node Transfer](docs/TRANSFER.md) | Discovery, pairing, idempotency, diagnostics |
| [Cloudflare Tunnel](docs/DOCKER_TUNNEL.md) | HTTPS/tunnel deployment |

## License

EndlessVibe is released under the [MIT License](LICENSE).
