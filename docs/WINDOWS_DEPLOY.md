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

## Phase B: actual service deployment

**Not enabled by Phase A.** An administrative capability for confirmed,
hash-bound, independently recoverable service switching needs a separate
review and implementation. The existing Windows PowerShell deployment script
remains the installed workflow; it performs an atomic manifest update
before stopping the service and can attempt rollback.

Until the dedicated native service-update action is implemented, run locally:

```powershell
cd F:\project\EndlessVibe
.\deploy\windows\EndlessVibe.ps1 -Action Update
.\deploy\windows\EndlessVibe.ps1 -Action Status
```

Do not enable `run_shell`, broad `trusted_host` access, or PowerShell in
`execution.allowed_programs` merely to deploy EndlessVibe.
