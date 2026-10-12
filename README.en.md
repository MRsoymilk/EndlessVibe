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
| **Dashboard** | First-run Setup, ChatGPT connection help, runtime charts, Projects, Tasks, Operations, Nodes, configuration, and storage |
| **Authentication** | OAuth 2.0 Authorization Code, PKCE, dynamic client registration, tool scopes |
| **Node Transfer** | LAN discovery, TLS pairing, scoped grants, persistent remote request status and history |
| **Execution Profiles** | Linux Bubblewrap and native offline Windows AppContainer, with explicit Project authorization |
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
- Linux requires Bubblewrap and unprivileged user namespaces by default. Windows offers a native offline `appcontainer` backend; new Windows/macOS configurations still disable command execution until a backend is selected. Unsandboxed host execution requires explicit acknowledgement

Linux refuses to run as root. Capability-based filesystem access applies to all supported builds; **Bubblewrap is Linux-only, and the Docker Unix Socket transport is not implemented on Windows**. See [Cross-Platform Support and Limitations](docs/PORTABILITY.md).

### 1. Build

From the repository root:

```bash
cargo build --release
cargo test --locked
```

### 2. Initialize using the dedicated wizard

On a new machine run `endlessvibe --init` (or `./target/release/endlessvibe --init` from a source build). On Windows use:

```powershell
.\target\release\endlessvibe.exe --init
```

This launches **only** the local initialization wizard at [http://127.0.0.1:20001/](http://127.0.0.1:20001/). It does **not** start public MCP/OAuth, LAN Transfer, or the normal Dashboard while the wizard is open. A missing configuration and owner key are created with private permissions, without granting any Projects.

The wizard has four steps across the top. Only the current step's configuration is displayed: **Public domain → Workspace → Project authorization → Finish**. The Next button unlocks when that step has been saved. The wizard is local-only; never reverse-proxy port `20001`.

### 3. Complete the four wizard steps

1. **Public domain:** provide your own HTTPS origin or a domain assigned by ngrok / Cloudflare Tunnel. Choose whether the tunnel runs on the host or in Docker. The wizard previews the MCP URL and proxy upstream and saves only validated origins.
2. **Workspace:** choose an ID and an existing absolute directory. A Workspace itself grants no Project permissions.
3. **Project authorization:** select a Workspace and add a specific existing project directory. Read-only is the default; write, command and Git Commit access must be explicitly selected.
4. **Finish:** read the generated normal startup command, copy the `https://your-domain/mcp` URL and follow the ChatGPT OAuth instructions.

For example, after configuring your own ngrok account, a host tunnel can use:

```bash
ngrok http 20000 --url https://your-domain.ngrok.app
```

Use your *actual* assigned domain and the configured MCP port; never expose the local Dashboard port `20001`.

### 4. Exit initialization and launch the normal service

After completing the wizard, stop the `--init` process (Ctrl+C or its original service manager) and launch the binary **without `--init`**:

```bash
endlessvibe
```

Windows source-build equivalent: `.\target\release\endlessvibe.exe`. The saved config and owner key are retained. **The normal Dashboard has no Setup tab or `/setup` API routes**. Reconfiguration requires stopping the service and explicitly running `--init` again.

The page distinguishes saved connection settings from active state and explains startup overrides. Check `ENDLESSVIBE_BIND`, `ENDLESSVIBE_PUBLIC_URL`, `--bind`, and `--public-url` if the normal service does not use the saved address. A public proxy still needs the MCP and OAuth routes; the wizard itself never verifies external reachability.

### 5. Connect ChatGPT

Once the HTTPS and OAuth endpoints are available:

1. Open [ChatGPT Plugins](https://chatgpt.com/plugins), then **+ → Add custom MCP server**.
2. Enter a name, description, and `https://your-domain/mcp`. Choose OAuth; EndlessVibe uses its existing dynamic client registration (DCR).
3. Complete the confirmation prompts, choose **Create as a plugin**, review the discovered tools, and install.
4. When prompted, enter your owner key only in **EndlessVibe's OAuth authorization page in your own browser**. No OpenAI API key is needed.
5. Start a new conversation, select EndlessVibe with `@`, and try listing authorized projects.

**Saving a domain or local configuration does not establish a ChatGPT connection.** Complete plugin installation, OAuth consent, and a real tool call. Setup's local checks do not claim to verify public reachability. Keep owner keys, tokens, and login credentials out of MCP URLs and chat messages.

For current UI and account policies, see OpenAI's [connection guide](https://developers.openai.com/plugins/deploy/connect-chatgpt) and [authentication guide](https://developers.openai.com/plugins/build/auth). Other MCP clients use the same HTTPS `/mcp` endpoint and OAuth; see [EndlessVibe OAuth](docs/OAUTH.md) for server details.

### Reopen Setup

If the executable is on PATH, run this whenever you need the guide again:

```bash
endlessvibe --init
```

On Windows, run this from the repository root without first adding the executable to PATH:

```powershell
.\target\release\endlessvibe.exe --init
```

For a custom configuration, add `--config`. This Windows example uses a placeholder path; replace it with your actual configuration file:

```powershell
.\target\release\endlessvibe.exe --init --config "C:\path\to\config.toml"
```

If the normal service is running, stop it first (with `--stop` or the original service manager); `--init` refuses a concurrent instance. When stopped, `--init` runs the dedicated wizard only. Configuration, owner keys, Workspace / Project grants, and comments are preserved. Reopening setup does not reset installation data.

### Optional: explicit CLI initialization

Before a configuration exists, the original command-line workflow remains available. Explicit `--init --workspace ...` creates the configuration and exits; existing configurations are never overwritten:

```bash
mkdir -p "$HOME/projects"

./target/release/endlessvibe \
  --init \
  --workspace "projects=$HOME/projects" \
  --public-url https://mcp.example.com

# The project directory must exist; --add-project requires a stopped service.
./target/release/endlessvibe \
  --add-project "projects:website=$HOME/projects/website"

./target/release/endlessvibe
```

Projects must be inside the registered Workspace root. Use `--read-only` or `--no-exec` to reduce permissions during registration. With an existing Linux configuration, run `./target/release/endlessvibe --check-sandbox` when needed to inspect Bubblewrap and toolchains. Diagnostics, credential maintenance, and process-management commands do not initialize a missing configuration.

## LAN-only mode (no public ChatGPT URL)

Use **`--lan-only`** to connect EndlessVibe instances via **TLS 1.3 Transfer pairing** without exposing MCP or OAuth over plaintext LAN HTTP.

**New Windows installation** (the directory must already exist):

```powershell
.\target\debug\endlessvibe.exe --init --lan-only --workspace "projects=C:\path\to\projects"
```

**Windows installation with an existing configuration** (including web setup; preserve the config and owner key):

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

Native Windows deployment now exposes three MCP tools: `windows_service_update_plan`, `windows_service_update`, and `windows_service_update_status`, **disabled by default** and bound to one explicit administrator-approved Project. A confirmed update requires both reviewed SHA-256 hashes and a unique request_id; a native detached helper performs graceful shutdown, version switching, and best-effort rollback without PowerShell. Bootstrap the new Windows child and Linux parent MCP builds once before using the tools. See [Windows deployment security](docs/WINDOWS_DEPLOY.md).

### Native Windows command sandbox

On recognized Windows 10/11 x64 kernels, select `appcontainer` under **Sandbox → Execution backend** in the local Dashboard and restart the service to run authorized command Jobs in Windows LPAC. Native tools, Cargo builds/tests, live stdout/stderr, job summaries, cancellation and timeouts use the regular Jobs interface. Each task has a fresh identity and Job Object. The entire process tree must exit before Project permissions and the private profile are reclaimed. Saved configuration and the active backend are reported separately; existing installations do not switch automatically. After a crash, a valid `running` record from the current Windows boot blocks recovery until a full Windows restart changes the verified boot sequence; `prepared` / `empty` records can recover after a service restart. Missing, malformed or untrusted evidence remains invalid after a Windows restart, and recovery is still refused.

The authorized Project is writable, toolchains are read-only, and an existing `.git` remains read-only unless Git mutation is authorized. Every AppContainer Job stays offline, including `development` Projects. Cargo uses only cached, checksum-verified public dependencies in a fresh private home, without host Cargo configuration or credentials. Required missing dependencies must be prepared in a trusted environment. The service's MCP, LAN Transfer and host ngrok connections are separate from this Job network policy.

Native Rust tests cover file boundaries, Git restrictions, inherited handles, descendant termination, permission recovery and an actual offline Cargo compile/link/test through Jobs. Preparation and cleanup errors are explicit; there is no fallback to host execution. See [Execution](docs/EXECUTION.md#windows-原生-appcontainer) and [Platform support](docs/PORTABILITY.md) for toolchain permissions, link handling, recovery and offline limits.

The separate fixed-worker self-check remains available as `.\target\windows-stage\debug\endlessvibe.exe --probe-windows-sandbox` after stopping the service. It validates the fixed `hash_input` protocol, native token, Job Object, bounded logs and recovery record. Its trusted-worker digest, PE imports, System32 mapping and offline embedded-signature diagnostics retain their documented limits and do not substitute for general command Jobs tests.

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

Default paths depend on the platform:

| Platform | Configuration | Private state |
| --- | --- | --- |
| Linux | `~/.config/endlessvibe/config.toml` | `~/.local/state/endlessvibe` |
| Windows | `%APPDATA%\EndlessVibe\config.toml` | `%LOCALAPPDATA%\EndlessVibe` |
| macOS | `~/Library/Application Support/EndlessVibe/config.toml` | `~/Library/Application Support/EndlessVibe/state` |

Linux/macOS respect `XDG_CONFIG_HOME` and `XDG_STATE_HOME`; Windows uses `APPDATA` and `LOCALAPPDATA`. Use `--config` for a custom configuration path. Runtime `ENDLESSVIBE_BIND` and `ENDLESSVIBE_PUBLIC_URL` values override file settings, and command-line address options take precedence over those environment variables.

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
| `isolated` | Restricted environment using the configured Bubblewrap or Windows AppContainer backend |
| `development` | Trusted development projects subject to the selected backend; Windows AppContainer always stays offline |
| `trusted_host` | **High-risk opt-in:** with an acknowledged Host backend, run locally installed executables on PATH |

`trusted_host` is **not enabled by default**. Switching the **global** execution backend to Host removes sandbox isolation for other command-enabled Projects too. Use it only in environments you control.

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
