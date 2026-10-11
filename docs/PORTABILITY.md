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

### Windows sandbox status in the local Dashboard

The /sandbox page has a Check Sandbox action on Windows as well as Linux.
On Windows this is a policy/status check: it does not execute Project code,
launch a sandboxed task or claim that AppContainer is production-ready.
It reports whether Job Objects resource limits apply to currently enabled
command jobs, marks filesystem/network isolation as absent for host mode,
shows the fixed-operation AppContainer worker protocol version and native
self-check CLI, and reports the protected startup profile-recovery facility.
AppContainer remains unselectable as a general command execution backend.

### Native fixed AppContainer self-check (Windows)

To validate the real lowbox token, zero network capabilities, Job Object
containment, versioned hash_input request and independently verified receipt,
stop the Windows EndlessVibe service and run the **separate** local command:

```powershell
# Run from the EndlessVibe project root. Do not overwrite the running EXE.
cargo build --locked --target-dir target/windows-stage
.\target\windows-stage\debug\endlessvibe.exe --probe-windows-sandbox
```

The CLI takes only an optional `--config ABSOLUTE_PATH`, and requires the
service's exclusive `service.lock` so recovery cannot race with another
instance. It never starts an MCP listener, launches Project code, reads
the Owner Key, changes Workspace grants, or enables `execution.backend`.
Only the private, non-secret probe fixture is staged. The worker entrypoint
is hidden and refuses a normal host-user token. Output reports
`status=passed`, token/Job Object verification and the hashed receipt;
failed checks return nonzero. The Windows Dashboard reports the availability
and restrictions but does not run the self-check automatically.

### Trusted Worker executable staging and verification

The fixed-operation self-check now copies the **currently running EndlessVibe
EXE only** into the unique private AppContainer profile. It does not accept a
Project executable path, script, shell, external EXE or user-defined command.
The source is read through one pinned Windows file handle, is required to be
a regular non-reparse file and has an upper bound of 128 MiB. Windows Cargo
builds may hardlink the running EXE to its build artifact; that does not
invalidate the source handle. The newly staged copy, however, must have
exactly one filesystem link and must not be a reparse point.

The broker computes SHA-256 from the same pinned source handle used for
copying, verifies the private copy before process creation and again after
the contained worker exits. The diagnostic response exposes only the SHA-256,
byte count and explicit trust boundary, not the host executable path.
Profile deletion and removal of the protected recovery journal must succeed
before a self-check reports status=passed. Native cleanup retries transient
Windows image-section release failures within a bounded interval; persistent
failure retains the journal for exclusive-startup recovery and returns an
error rather than reporting successful cleanup.

### Bounded PE dependency preflight (Windows)

Before the current trusted Worker EXE starts inside AppContainer, the broker
parses its PE32+ AMD64 static import directory with explicit file/section/RVA
bounds. It rejects unknown DLL names, imports containing path syntax, malformed
or overlapping PE sections, missing descriptor terminators, and delay-load
imports. Allowed static names are fixed Windows OS/API-set contracts and a
small, separately classified MSVC redistributable list. For MSVC DLLs such as
vcruntime140.dll, the broker obtains the System32 location via the native
GetSystemDirectoryW API (never PATH or a user-provided SystemRoot), rejects
reparse-point DLL files, and hashes the System32 runtime before and after
the fixed Worker runs. The fixed executable copy is still verified by SHA-256.

This is a **static dependency-name preflight**, not full Windows loader
attestation: Windows may resolve transitive or forwarded imports and OS DLL
loading rules are more complex than import table inspection. No general
sandbox command runner is enabled by this stage. The diagnostic response
reports the approved module names, separately identified System32 MSVC
runtime modules and explicit limits, without revealing local file paths.

**This does not authenticate arbitrary executables** or provide a secure
loading path for user code. A future general sandbox backend still needs
a separate executable identity/allowlist policy, verified dependencies,
cancellation, quotas, filesystem boundaries and security review.

Production support remains limited to the fixed hash_input self-check;
general AppContainer command execution is not available.
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

The fixed hash_input worker is now compiled into the Windows executable and
can run only within a verified AppContainer process. This does not make it
a production command-execution backend: cargo, arbitrary Project binaries,
scripts, shell commands and general file writes remain unsupported.

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
The restricted worker instead opens its private staging directory as a
capability handle (without canonicalize), resolves only relative, validated
inputs/... paths and rejects reparse-point or hard-linked inputs. The broker
continues to use its secure capability-rooted Project APIs. Native tests may
stage authorized Project files; the production CLI self-check stages only a
synthetic private sample, not user Project content.

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

The **production fixed hash_input self-check** now has a bounded log broker:
the lowbox worker creates separate private `child-stdout.log` and
`child-stderr.log` files, while its privileged parent reads them in 4 KiB
pages through capability-held directory handles. A **combined 64 KiB limit**
applies to both files; previously consumed prefixes are SHA-256 checked on
every poll and checked again after the Job Object has exited. The parent
terminates the whole Job Object if the budget is exceeded or any streamed
prefix changes, and refuses missing/truncated/replaced final logs. The
CLI returns only bounded byte counts and SHA-256 digests, not arbitrary
worker output or input content. A separate test uses a deliberately noisy
AppContainer process to verify over-budget termination.

This is a polling limit, **not a filesystem disk quota**: a burst may
temporarily exceed 64 KiB between polls. The existing Windows test harness
also demonstrates file-backed live stdout/stderr for cooperating processes.
Neither facility is transparent capture of arbitrary external executables;
the generic `run_command` path still uses explicitly acknowledged host mode
and does **not** inherit the fixed worker's AppContainer protection.
The Windows service now runs an orphaned-profile recovery scan during
Runtime::new, after obtaining the exclusive service.lock and before
accepting any commands. The private recovery directory lives under
security.data_dir/appcontainer-profiles. All journal entries must match
the exact EndlessVibe-owned name, regular-file type and bounded record
contents; the complete batch is verified before any native profiles are
deleted. Cleanup consumes each journal only after successful deletion.
Malformed journals fail closed rather than deleting unrelated profiles.

Native Windows tests share this production recovery implementation and
simulate interrupted service cleanup by leaving profiles registered.
The production fixed-operation self-check now journals every AppContainer
profile *before* creating it. Normal completion removes the record only after
native profile cleanup; after an interrupted CLI probe, the next exclusive
service startup can recover and remove the orphaned profile.
The previous CreateProcess cross-token handle inheritance experiment
failed native validation and was discarded. General-purpose command
execution, crash-safe job lifecycle and filesystem quotas are still
separate work; the current fixed-operation protocol is not an arbitrary
Project command runner.

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
