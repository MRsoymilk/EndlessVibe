# Windows service self-update architecture

## Phase A: authorized read-only preflight

`windows_service_update_plan` is a **read-only** MCP/paired-node operation.
It does not stop, start, update or spawn the EndlessVibe service.

The preflight is deliberately **disabled by default**. In a Windows child
configuration the administrator can explicitly add:

```toml
[security]
allow_windows_service_update = true
windows_update_project = 'F:\project\EndlessVibe'
```

The project must already be registered, grant **write and execute**, and its
canonical directory must match the one configured above. Paired-node requests
also require a per-Project **execute** grant; a read-only remote Project grant
cannot initiate this preflight. OAuth callers require
`commands:execute` and `files:write`. The result never authorizes a command.

The only candidate path is fixed in the native implementation:
`target/windows-stage/debug/endlessvibe.exe`, relative to that Project.
It checks every parent for Windows junctions/reparse points, refuses
non-regular files, limits the EXE to 128 MiB, hashes the exact bytes, and
requires the existing PE32+ AMD64 import allowlist to succeed. The running
binary is accepted only from the versioned
`Programs/EndlessVibe/releases/<sha256>/endlessvibe.exe` directory and its
`current.json` manifest must agree with its actual bytes.

The tool reports current/candidate SHA-256, input bytes and whether an update
would change the binary. No paths or hashes supplied by remote callers select
any executable, service name, arbitrary installer or PowerShell process.

## Phase B: confirmed native service update

Three tools are implemented: `windows_service_update_plan` (read-only),
`windows_service_update` (explicit action) and `windows_service_update_status`
(read-only, recovery). They require the Windows administrator's opt-in and
one exact Project with write+execute rights. Remote calls require an
**execute Project grant**, plus OAuth commands:execute and files:write scopes.

After reading a fresh plan, request an update using the exact workspace,
project, a **new stable request_id**, `confirm=true`, and both full
`expected_current_sha256` / `expected_candidate_sha256` from the plan.
No arbitrary executable path, shell, service name or command is accepted.

The installed service checks the fixed target/windows-stage/debug/endlessvibe.exe,
copies it to a versioned release directory, verifies its exact SHA-256 and
launches an independent **native EndlessVibe-only helper** with one-use
private authorization. The helper revalidates the old/new digests, service
PID and creation time, the administrator's exact Project grants and opt-in.
One-time helper authorization expires after five minutes. An exclusive job
admission guard refuses an update while a job is being submitted, running,
or completing its final cleanup. Once acquired, it rejects new jobs across
all Projects throughout the helper handoff. Requests that reuse an existing
request_id can still retrieve that job without executing it again.
It gracefully stops the old instance, starts the verified candidate, checks
the new process identity, then atomically updates current.json with a
nonempty backup path. It attempts to recover the previous installed release
if startup or manifest publication fails, but **never force-kills** processes
or starts a second instance while a service lock remains held. A failed new
process must be confirmed exited before an old release can be started.
Manifest replacement failure is checked explicitly: the old manifest must
still be valid or be restored from a verified backup before rollback.

A candidate or recovery process starts with job admission paused while its
update transaction is unresolved. Admission resumes only after a complete
terminal record and the running executable agree with the final manifest.
Unknown recovery state stays paused for inspection; normal shutdown closes
admission permanently for that process.

After a disconnect, query `windows_service_update_status` with the original
request_id to see pending, in_progress, succeeded, rolled_back, failed,
recovery_required, or unknown.
**Do not replay an update on a lost response.** Results are recorded under
the private windows-service-updates directory. A terminal result is written
and synced in a temporary file, then published without replacing an existing
result. The service monitor removes the working record only after verifying
the terminal result, the helper's exit, and the installed service identity.
Write, sync, or publication failure preserves recovery evidence;
an abandoned working record is not reported as a running helper indefinitely.

### One-time bootstrap and limitations

The current running Windows service and the Linux parent must first load
a build containing these MCP tools; the old binaries cannot expose them.
Bootstrap Windows locally from PowerShell:

```powershell
cd F:\project\EndlessVibe
.\deploy\windows\EndlessVibe.ps1 -Action Update
.\deploy\windows\EndlessVibe.ps1 -Action Status
```

Then set the explicit [security] fields above in the installed Windows config
and restart to activate opt-in. Update the Linux parent EndlessVibe separately
for the new MCP tool catalog to be available from ChatGPT.

The native helper's rollback is **best effort**. If another process holds
service.lock or PID identity is uncertain, it fails closed and may require
manual recovery. Verified process startup is not an application-level health
check. This tool does not create a general AppContainer command sandbox or
validate Authenticode publisher identities.

Do not enable run_shell, trusted_host, or PowerShell command access solely
to deploy EndlessVibe.
