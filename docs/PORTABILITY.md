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
| Native AppContainer command Jobs | Not available | Not available | LPAC, offline, per-job ACL leases and Job Object |
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

Windows host command Jobs start their child suspended, attach a native Job
Object, then resume the original thread. Each task uses the configured
execution.memory_limit_mb and execution.max_processes to cap aggregate Job
memory and active processes. Closing the Job Object terminates remaining
children, including on cancellation, timeout, and completed commands.
Assignment failures abort the suspended process rather than running outside
the resource limits. The old taskkill-based cleanup is not needed for these
managed command Jobs.

Job Objects provide resource and process-tree containment. The `appcontainer`
backend adds LPAC token and per-job filesystem permissions; host mode keeps
the service account's file and network access and requires explicit unsafe-host
authorization.

### Windows sandbox status in the local Dashboard

The `/sandbox` page checks native tool availability and displays the selected
and active execution backends. Windows can select `appcontainer` for general
command Jobs. The policy check does not execute Project code; saving a backend
requires restart before it becomes active. Host mode explicitly reports absent
filesystem and network isolation.

### Native AppContainer command Jobs

On recognized Windows 10/11 x64 kernels, the backend executes configured native
programs and their descendants inside a per-job LPAC identity. Process creation
atomically joins a memory/process-limited Job Object through the startup job-list
attribute. The suspended token and Job membership are verified before resuming.
The sole added capability is `registryRead`; no network capability is granted.
The authorized Project is
read/write, exact toolchain roots are read/execute, and an existing `.git` is
read-only unless the Project permits Git mutation. There is no host fallback.

ACL changes are journaled before grants. Paths are pinned without following
reparse points; external or conflicting hardlinks fail preparation. Three fresh
standard pipes carry stdin/stdout/stderr; host credentials and arbitrary
handles are not inherited. Jobs retain live output, cancellation, timeout,
summary capture and restart-interrupted status. Normal completion also kills
orphaned descendants. Verified whole-Job exit precedes ACL/profile cleanup.
Unverified cleanup retains recovery records, blocks new Jobs and keeps the
Project lock. Authorization reload cannot replace Project locks during command
submission, running Jobs or unresolved cleanup. A service-instance lock alone
does not prove old descendants have
exited. Per-identity prepared/running/empty records are checked before any ACL
recovery; every pending ACL SID must have matching proof. A `running` record
from the same Windows boot blocks recovery, including after a service restart.
A full Windows restart with a different verified kernel boot sequence permits
recovery; prepared/empty records need only a service restart. Unknown shared-data
ABIs, missing proof and malformed records fail closed. ACLs are recovered before
profiles, and execution evidence is deleted last.

All profiles, including `development`, remain offline in this backend.
`network=true` is rejected. MCP/Transfer and host tunnels remain separate
service processes. A private Cargo home receives only verified, locked public
crates.io cache data; host Cargo configuration, credentials and Git registry
caches stay outside it. Ordinary tool roots require the exact permissions
needed for a temporary read-only ACL lease. Automatically detected MSVC/SDK
roots with a selected leaf lacking ACL-editing permission trigger copying of
that family's already
authorized native subdirectories into a bounded per-job private tool tree,
without changing the installed source ACL or owner. Other families stay in
place. Selected relative layout and executable/library/header paths are
preserved or remapped; a conflicting explicit in-place mount is rejected. The copy
is sealed with effective read/execute grants and protected inheritance, while
profile Temp and cache siblings remain writable. Preparation checks a separate
90-second deadline and refuses to launch after it expires. See
[Windows configuration, toolchains and limits](EXECUTION.md#windows-原生-appcontainer).

No disk quota is provided. Reparse points introduced during a Job make cleanup
fail closed rather than traversing them. This model does not claim to contain
an administrator or another trusted host process running as the service user.
Existing installations keep their configured backend until explicitly changed
and restarted.

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

### Live AppContainer module mapping checks (Windows)

The production fixed hash_input Worker now uses two short, bounded, private
file-backed handshakes. It stops after the Windows loader reaches the trusted
lowbox entrypoint, **before reading its staged input**; the privileged parent
uses Toolhelp to inspect the *actual running child PID's* mapped images. It
requires exactly the hash-verified fixed Worker EXE and DLL mappings directly
under the canonical Windows System32 directory. The parent refuses duplicate
basenames, unexpected paths, non-regular/reparse-point mappings and missing
module records. It writes the fixed allow marker only after that check.

After the Worker finishes its fixed hash operation, it waits at a second
barrier while the parent repeats the same live inspection; only after that
check does the Worker exit. Parent-detected violations abort the entire Job
Object. Missing, modified or timed-out handshake markers also fail closed.
The command reports two sanitized module-name snapshots and system DLL counts,
not the workstation's absolute DLL paths.

For each Toolhelp module base address, the broker also asks the kernel for the
mapped section's backing file name using K32GetMappedFileNameW. QueryDosDeviceW
resolves the drive-letter mapping to the NT device path; the resulting name
must agree with the independently canonicalized Toolhelp file path at both
inspection barriers. Unexpected, truncated, non-local and mismatching device
names are rejected. Native tests verify valid mappings and reject forged
Toolhelp paths and invalid mapped base addresses.

### Pinned loaded-file identity checks

The parent now holds a real Windows file handle for every observed image
across both loader checkpoints. GetFileInformationByHandle records a nonzero
volume serial/file index, file length and last-write FILETIME; the opened
handle's resolved path must agree with a separately canonicalized Toolhelp
path. Each previously pinned handle is checked again after the fixed
operation, and matching module names must have identical identity/metadata
in both snapshots. Same-name file replacement, in-place size changes,
unresolvable handles and reparse-point images fail closed. These opened
handles remain valid until the broker finishes verification and are released
before deleting the temporary AppContainer profile.

**Scope:** This pins the *reopened filesystem files* identified by their
paths and protects against path replacement across the two checkpoints.
GetMappedFileNameW still separately confirms that the kernel-reported
mapped-section path matches the Toolhelp path; documented Win32 APIs used
here do not expose the original image section's underlying file ID.
Consequently these checks do not cryptographically prove the exact bytes
mapped in process memory, validate Authenticode signatures, or detect a DLL
loaded and unloaded between snapshots. General run_command execution remains
outside AppContainer and is not authorized by this fixed-operation check.

### Offline embedded Authenticode diagnostics

The fixed hash_input self-check also collects **diagnostics only** for its
preapproved System32 MSVC runtime DLL imports. The privileged broker opens
each runtime without following Windows reparse points and invokes the
WINTRUST_ACTION_GENERIC_VERIFY_V2 trust provider via WinVerifyTrust, passing
the opened file handle. The call uses WTD_UI_NONE and
WTD_CACHE_ONLY_URL_RETRIEVAL: trust-chain checks do not perform online
certificate or revocation downloads.

A successful embedded trust-provider verdict is reported separately from
a missing/untrusted embedded signature. Many genuine Windows DLLs are
**catalog-signed rather than embedded-signed**, so a failed embedded-only
check is not treated as proof of a malicious or unsigned DLL. The response
returns only approved DLL basenames and diagnostic verdict labels, never
absolute machine paths or certificate material.

**Not an execution gate:** this stage does not validate system Catalog
membership/signatures, pin a Microsoft publisher identity, implement an
offline revocation policy, or authorize arbitrary executables. Existing
AppContainer token, PE import whitelist, pinned file identity, and two
module-gate checks remain the enforced boundary. Catalog and signer-policy
verification must be reviewed before any signature result can authorize a
new execution backend.

Microsoft reference: WinVerifyTrust (wintrust.h),
https://learn.microsoft.com/windows/win32/api/wintrust/nf-wintrust-winverifytrust ;
WINTRUST_DATA offline flags,
https://learn.microsoft.com/windows/win32/api/wintrust/ns-wintrust-wintrust_data .

### Native-only lowbox startup environment

The fixed Worker no longer inherits `SystemRoot`, `SystemDrive`, `WINDIR`
or `PATH` from the privileged service process. The broker obtains System32
through the Windows GetSystemDirectoryW API, derives the matching Windows
root and drive prefix, and sets PATH to that System32 directory only. Private
AppContainer profile paths are used for TEMP, TMP, LOCALAPPDATA, APPDATA and
USERPROFILE. `ComSpec` is not exposed and no shell path is needed for
the fixed hash operation.

Extra worker environment keys must use the explicit uppercase
`ENDLESSVIBE_` namespace; case-insensitive duplicate names, missing key
prefixes, unsafe characters and oversized values fail closed. Parent-level
execution environment and network flags are not silently forwarded. Native
tests inspect the exact Windows wide-string environment block, and verify
that this change preserves the AppContainer identity, file/network restrictions
and two-gate loaded-module checks.

These fixed-worker checks authenticate only the staged EndlessVibe worker.
General command Jobs use the separate AppContainer backend described above,
which treats the executable and its Project code as untrusted and confines
them through the OS. Fixed-worker module attestation is not reported for
arbitrary programs.

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
can run only within a verified AppContainer process. Its fixed protocol still
accepts no arbitrary command. General programs use the separate command Jobs
backend, with their normal authorization, output and lifecycle contracts.

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

The Windows-only fixed AppContainer worker creates an unpredictable one-time
profile with no network capabilities and uses the Win32 extended startup
attributes to launch a suspended, low-privilege process. Native tests inspect
its access token and confirm that it actually has AppContainer identity.
Temporary profiles are deleted after their process has exited.

The fixed AppContainer input staging reads only individually selected
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
These fixed-worker file logs remain separate from the anonymous pipes used
for general AppContainer command stdout/stderr. `run_command` uses the active
configured backend; acknowledged host mode remains unsandboxed.
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
General command Jobs use their own verified native process launcher with an
explicit three-handle inheritance list. Their ACL recovery runs before this
profile recovery under the same service instance lock. Newly created journal
directories have protected native DACLs; pre-existing directories and records
with untrusted write permissions are rejected. Filesystem disk quotas remain
unimplemented.

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
default to `execution.backend = "disabled"`. On Windows, select `appcontainer`
to enable native offline command Jobs. On either platform, selecting `host`
requires `acknowledge_unsafe_host_execution = true` and runs executable code
with the service account's OS privileges without sandbox protection.

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

Run the full `cargo test --locked` suite on each supported OS separately.
Windows integration fixtures use native test executables and include real
Jobs-based AppContainer execution and an offline Cargo compile/link/test.
The native boundary tests verify file restrictions, inherited handles, process
trees and ACL recovery. Do not interpret Linux checks as Windows or macOS
validation. A test that needs an installed toolchain must report its absence
as a failure rather than silently skipping the execution claim.

The local Dashboard binds to `127.0.0.1:20001` and should not be exposed
via a public reverse proxy. MCP itself requires correct OAuth settings and
a suitable HTTPS reverse proxy when accessed over the public internet.
