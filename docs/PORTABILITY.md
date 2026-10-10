# Platform support

EndlessVibe targets **Linux, Windows and macOS**. This is a portability
implementation in progress. Each operating system must be compiled and
tested independently before support can be considered verified.

| Feature | Linux | macOS | Windows |
| --- | --- | --- | --- |
| OAuth / MCP HTTP server and local Dashboard | Supported | Portable implementation | Portable implementation |
| Workspace / Project file APIs | Linux `openat2` | `cap-std` directory handles | `cap-std` directory handles |
| SQLite / task and audit records | Supported | Portable implementation | Portable implementation |
| Git file/diff/commit workflows | Supported | Portable implementation | Portable implementation |
| Bubblewrap execution | Supported (if installed) | Not available | Not available |
| Host command execution | Explicit opt-in | Explicit opt-in | Explicit opt-in |
| Built-in `--status` / `--stop` / `--restart` | Linux PID start-ticks verification | Use OS process manager | Windows creation-time verification + per-instance shutdown event (native validation pending) |
| Docker Engine Unix-socket integration | Optional | Depends on Unix socket | Unix-socket transport unavailable |

**Important:** a successful Linux build is *not* proof that either other
operating system builds successfully. Native builds and unit tests must be
run on Windows and macOS before a release is described as supporting them.

### Windows service restart

After upgrading from a Windows build that lacked process control, **stop the
old process once using Ctrl+C or the service manager**, then launch the new
`endlessvibe.exe` normally. Older processes do not have `service.pid` or the
Windows shutdown event, so the new CLI refuses to force-kill them.

Subsequent invocations support `endlessvibe.exe --status`, `--stop`, and
`--restart` with the same configuration. Windows stores a PID and its actual
Windows process creation timestamp in the private state directory, then
creates an OS event scoped to the state directory, PID and start timestamp.
The CLI verifies the process identity before signaling that event. The service
handles it through the normal graceful shutdown path; `--stop` waits for exit
and `--restart` starts a new instance only after the old one exits. If a
process has no valid identity/event, or fails to exit within 15 seconds, the
command **fails closed** without `taskkill /F` or terminating by process name.

The control event uses Windows' `Local\\` namespace: stop/restart are intended
for the same desktop logon session and Windows account. Sessions managed by
external service supervisors should still be controlled by those supervisors.
No host-command execution backend needs to be enabled for process control.

### Windows Job Object resource containment

Windows command Jobs now start their child suspended, attach a native Job
Object, then resume the original thread. Each task uses the configured
execution.memory_limit_mb and execution.max_processes to cap aggregate Job
memory and active processes. Closing the Job Object terminates remaining
children, including on cancellation, timeout, and completed commands.
Assignment failures abort the suspended process rather than running outside
the resource limits. The old taskkill-based cleanup is not needed for these
managed command Jobs.

Job Objects do not restrict file access, registry, network or user security
tokens: this is a resource-containment stage, NOT a Windows AppContainer
sandbox. Host mode still requires explicit unsafe-host authorization.

### Versioned isolated-worker protocol

src/tools/sandbox_protocol.rs defines a size-bounded, strict JSON request
and receipt protocol. Version 1 currently permits only a single built-in
hash_input operation on an explicitly staged inputs/... relative path,
not an arbitrary program, command line, script or shell. Requests are at
most 4 KiB, paths reject Windows ADS syntax and directory traversal, and
input files are limited to 2 MiB. Receipts are at most 4 KiB; the privileged
broker must recompute the input SHA-256 using independently opened files
before it accepts the receipt. Unknown fields, versions and operations
fail closed.

This protocol is a security boundary prerequisite for a future AppContainer
worker, not a production command-execution backend. Passing a receipt does
not imply that cargo or another external executable can run.

### Verified AppContainer protocol round trip

The native Windows test harness now stages one explicitly authorized Project
file and a versioned JSON request in a fresh private AppContainer profile.
A real low-privilege worker (with its AppContainer token checked before
resuming) performs the single approved hash_input operation and writes a
bounded worker receipt to its private outputs directory. The parent reads
that receipt via its capability-pinned output broker and independently opens
the original authorized Project file to verify size, SHA-256, job ID and path.
Tampered receipts fail closed and never overwrite original files.

Windows denies the privileged workspace-root resolution inside AppContainer.
The restricted worker instead reads only its own pre-staged inputs through
read-only, non-reparse-point file handles, after validating the strict
inputs/... path grammar. The privileged parent continues to use its secure
capability-rooted Project APIs. These are native test-only workflows, not
an unrestricted command runner or selectable AppContainer execution backend.

### Windows AppContainer identity verification

The Windows-only AppContainer prototype creates an unpredictable one-time
profile with no network capabilities and uses the Win32 extended startup
attributes to launch a suspended, low-privilege process. Native tests inspect
its access token and confirm that it actually has AppContainer identity.
Temporary profiles are deleted after their process has exited.

The AppContainer input-staging prototype reads only individually selected
files via the existing capability-rooted Project reader. It refuses sensitive
names, symlinks, and traversal, and copies at most 2 MiB per file into the
ephemeral AppContainer profile. The original Project's ACLs are unchanged.
Native Windows tests launch a real AppContainer process: it can read the
staged file but cannot read the original Project file or reach the local
Dashboard over loopback when no network capabilities are granted.

The AppContainer test runner now supports per-task cancellation and timeout.
After verifying the actual AppContainer token and assigning the suspended
child to its Job Object, the runner resumes the process. On normal exit,
cancellation or timeout it terminates the entire Job Object and waits for
the contained process to exit. Native tests exercise both forced-stop paths.

The Windows-native output broker now prepares a private per-profile
outputs directory *before* launching the untrusted child. After the child
exits, the parent may explicitly collect one relative output path through
the capability-pinned file reader. Collection enforces sensitive-name and
traversal checks, rejects links, bounds each file to at most 2 MiB and
returns both its bytes and SHA-256 for review. Output collection does not
modify the original Project. Publishing into a Project requires a separate
authorized write_file operation with its normal expected SHA-256.

Windows native tests also verify an AppContainer-owned file-backed standard
output path. A cooperating restricted process creates its own private stdout
and stderr files and updates its own process-local standard handles; the
parent neither lends nor inherits service-owned handles. The privileged
broker can read the initial stdout data while that process is still running,
then review each completed file through the existing bounded, hash-checked
output broker.

This remains Windows test-only infrastructure, NOT a production execution
backend. It demonstrates streaming for a cooperating AppContainer process,
not transparent capture of arbitrary external executables. The Windows test-only log cursor delivers bounded byte chunks while
verifying SHA-256 of every previously delivered prefix. Truncated, replaced,
overlong, or disappearing logs fail closed rather than silently resuming.
The test monitor cancels the entire Job Object when a log exceeds the
configured total output budget. These are poll-time budgets, not filesystem
disk quotas: a burst may temporarily exceed the budget between checks.
A Windows test-only recovery journal now records the unique profile
identifier in a service-owned, private file *before* calling the Windows
profile creation API. On normal teardown the journal is cleared only if the
profile was deleted. The recovery prototype validates each marker's exact
namespace, file type, size and contents, then deletes that AppContainer
profile and consumes its marker. Windows tests simulate an interrupted
service without running destructors and reject forged recovery markers.
Only a service instance that has acquired its exclusive state-directory
lock, and confirmed no old jobs remain alive, may invoke such recovery.
Production startup does not invoke this recovery prototype yet. Hard
disk quotas and production job lifecycle remain separate work. The previous CreateProcess cross-token handle
inheritance experiment failed native validation and was discarded. A
production-safe command protocol, task lifecycle, and crash recovery remain
required before enabling arbitrary Project commands.

### Windows MSVC toolchain in host jobs

When a Windows host job executes cargo, rustc, cmake, or ninja, EndlessVibe
discovers an installed Visual Studio MSVC x64 toolset and a Windows SDK.
It injects absolute paths for link.exe, cl.exe, lib.exe, the SDK libraries
and include directories, along with PATH, LIB, and INCLUDE settings.
The newest complete installed versions are selected, not hardcoded releases.

For nonstandard installations, set ENDLESSVIBE_MSVC_LINKER to the absolute
path of link.exe in the service process environment before starting
EndlessVibe. Matching cl.exe, lib.exe, runtime libraries and a Windows SDK
must also be installed. Explicit job environment values override discovered
defaults. This fixes host toolchain setup; it does not add sandbox isolation.

## Safe defaults

On Linux, Bubblewrap is the default execution backend. Windows and macOS
default to `execution.backend = "disabled"`; command jobs are unavailable
until an administrator deliberately configures the `host` backend and sets
`acknowledge_unsafe_host_execution = true`. That mode runs executable code
with the service account's OS privileges and is **not** a sandbox.

File APIs use a capability-held Project directory on non-Linux hosts and
reject parent traversal, sensitive path components, and symbolic links.
Windows private state relies on the actual per-user ACL of the selected state
directory. Do not place configuration, owner keys or state under a
world-writable location; run as a dedicated unprivileged account.

The default configuration/state directories follow XDG paths on Unix;
Windows uses `%APPDATA%\\EndlessVibe` for configuration and
`%LOCALAPPDATA%\\EndlessVibe` for state.

## Local verification

```sh
cargo check --locked --all-targets
cargo test --locked
```

Run `cargo check --locked --all-targets` and `cargo test --locked --lib`
on each supported OS separately. Run the complete `cargo test --locked` suite
on Linux for the Linux-specific integration tests. Do not interpret passing
checks on Linux as proof of Windows or macOS compatibility.

The local Dashboard binds to `127.0.0.1:20001` and should not be exposed
via a public reverse proxy. MCP itself requires correct OAuth settings and
a suitable HTTPS reverse proxy when accessed over the public internet.
