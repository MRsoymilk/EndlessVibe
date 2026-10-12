# EndlessVibe Transfer / Parent–Child Nodes

## LAN-only deployment (no public ChatGPT endpoint)

To operate entirely through the encrypted LAN node mesh, initialize a new instance with `--init --lan-only --workspace ID=/absolute/path`. An already initialized instance may be converted **in place** by stopping EndlessVibe and running `endlessvibe --lan-only` (optionally with `--config PATH`). Existing owner keys, Workspace/Project grants, and configuration comments are preserved.

In this mode, `[server].lan_only = true`, local MCP binds to `127.0.0.1:20000`, and `server.public_url = "http://127.0.0.1:20000"` is **only an internal OAuth resource identifier**, not a public ChatGPT URL. `[security].allow_http_loopback = true` is used for the loopback OAuth service only. The Dashboard remains at `127.0.0.1:20001` and is not exposed over LAN. Transfer is enabled for TLS 1.3 at TCP `20002`, with optional multicast discovery via UDP `20003`. Configure Windows/macOS/Linux firewalls for a trusted private network.

On each computer, open `http://127.0.0.1:20001/nodes` locally, select the other node or enter its LAN IP and Transfer port, verify matching codes, and independently approve the peer and exact Project grants. Do **not** expose the loopback MCP service over a plain HTTP proxy: remote MCP HTTP clients require a separate HTTPS-protected deployment.

---
## Phase 1 — LAN Discovery

`[transfer]` is disabled by default. When enabled, discovery sends UDP beacons to `239.255.77.77:20003` on **each active private/link-local IPv4 interface**, selecting each interface explicitly rather than relying on the OS default multicast route. It also sends to that interface's **directed subnet broadcast** on UDP `20003` (for example `192.168.1.255` on a `/24` LAN) so peers may still be found when Wi-Fi multicast forwarding is suppressed. The receiver joins the multicast group on each available interface and refreshes memberships periodically after adapter changes. Already paired peers also receive a direct UDP location beacon on port `20003` using their pinned pairing record's LAN IP, even when multicast and directed broadcast forwarding are blocked. Discovery sends every 5 seconds; an unseen node becomes offline after 20 seconds. Interface enumeration and broadcast failures do not disable the encrypted TLS Transfer listener.

Only the stable node ID, display name, and declared Transfer port are advertised; **discovery packets are not authenticated and never grant access**. The node ID is persisted in the private SQLite store. A local-only `GET /api/nodes` endpoint exposes the discovered nodes. UDP discovery remains link-local and may still be blocked by VLANs, routers, AP/client isolation, or broadcast suppression. When TCP `20002` is reachable, parents can always **enter the child's LAN IP:port manually** on the Nodes Dashboard to pair over TLS; discovery is not required. A separate TLS 1.3 listener uses the configured `transfer.listen` endpoint, and all remote Project operations require an independent child-side Project grant.

```toml
[transfer]
enabled = false
listen = "0.0.0.0:20002"
advertise = true
discover = true
display_name = "EndlessVibe"
```

## Phase 2 — TLS Pairing and Authorization

The independent TLS 1.3 listener uses a persistent, self-signed per-node certificate stored in the service's private SQLite state. The parent starts pairing by POSTing `{"address":"192.168.1.20:20002"}` to its local `/api/nodes/pair`. The parent receives the child's TLS certificate fingerprint and a six-digit verification code. The child receives the request and the **same code** under `/api/nodes/pending`. The operator MUST compare the code shown by the parent with the code shown to the child, then click **Accept** or **Reject only on the child's Dashboard**. The parent does not have a separate approval step: it queries `pair_status` over the child's pinned TLS connection and completes pairing only after the child accepts. Encrypted transport alone does not authenticate an unpaired node. Only RFC1918/loopback/link-local IP endpoints are permitted; public internet addresses are refused.

The child's local Dashboard calls `POST /api/nodes/approve` with `{"id":"<pairing_id>"}` or `POST /api/nodes/reject` with the same ID. **The default is no Project grants.** The UI never asks the child to configure grants while approving a connection. Once paired, the child's Trusted Nodes panel can grant/revoke individual Projects separately; newly registered Projects are never auto-granted. Existing API clients may explicitly pass an optional `grants` array to approve, but the Dashboard sends none. The parent automatically stores the child's pinned certificate fingerprint and its randomly generated bearer token after authenticated status confirmation; the child stores only the token digest. Requests and rejection decisions expire in 5 minutes. The child refuses a second distinct parent until the first is revoked, and the parent cannot elevate privileges above the child's existing Project permissions. Peer records survive service restarts. `DELETE /api/nodes/peers/{node_id}` revokes local access. All Dashboard APIs are loopback-only, not part of the public MCP endpoint.

**Trust model:** During first pairing the TLS server certificate has no pre-existing trust anchor; the human comparison of the short code is therefore mandatory to rule out a local active MITM. After approval the parent pins the server certificate fingerprint and refuses unexpected key changes. An approved child accepts remote operations only with the matching parent ID and token plus its per-project grant. Protect the local Dashboard (e.g. do not expose 20001 to LAN) and use verified SAS comparison. Do not mistake multicast advertisements for cryptographic identity.

## Phase 3 — Nodes Dashboard and Remote Read-Only Routing

The local `http://127.0.0.1:20001/nodes` page distinguishes the local **Parent** and **Child** roles (or an unassigned node before its first request). Only a parent may initiate requests; a child sees incoming parent requests, verification codes, and one-click **Accept / Reject** controls. Parent-side pairing status is reconciled automatically every few seconds even when the parent Dashboard is closed. The page also includes discovered nodes, approved peers with a **Manage Project grants** editor on children, and a parent-only remote Project browser. Transfer can be enabled or disabled from `/config` with `PUT /api/config/transfer` (full revision check; restart required). The Dashboard remains loopback-only and the dedicated Transfer socket uses TLS 1.3.

`node_list` and `node_read` are the parent's MCP tools; the latter supports `list_workspaces`, `list_projects`, `inspect_project`, `list_directory`, `read_file`, `search_code`, `git_status`, `git_diff`, and `git_log` using the same parameter objects as the local equivalents. The parent is allowed only approved peers with their previously pinned TLS certificate fingerprint. The child verifies the parent token for every request, filters Workspace/Project listings by exact grants, and then invokes its existing local Project locks and filesystem/Git helpers. Arbitrary tool names, nested forwarding, paths outside the Project, and remote mutation attempts are rejected.

To update grants after pairing, the **child's local Dashboard only** accepts:

```http
PUT /api/nodes/peers/{parent_node_id}/grants
Content-Type: application/json
```

```json
{
  "expected_grants_revision": "64-character revision from GET /api/nodes/peers",
  "grants": [
    {"workspace":"project","project":"example","read":true,"write":false,"execute":false,"git":false}
  ]
}
```

This atomically **replaces** the full grant list. Set `"grants":[]` to revoke all Project permissions while retaining pairing. New Projects never become authorized automatically; permission changes are checked against the child’s current Project settings. A stale revision returns HTTP 409 to prevent lost updates. Every new remote request checks the latest grants. The paired parent cannot request extra grants from the child over Transfer.

For browser use the parent exposes loopback-only `POST /api/nodes/read` with `{\"node_id\":\"...\",\"tool\":\"read_file\",\"arguments\":{\"workspace\":\"...\",\"project\":\"...\",\"path\":\"src/main.rs\"}}`.

### Cached ChatGPT MCP tool catalogs (legacy 20-tool clients)

Some clients retain the pre-Workspace/Project MCP tool signatures and do not expose
the later node_list, node_read and node_write tools even though the server
publishes all of them in tools/list. Reauthorizing such a connection does not
necessarily refresh its tool catalog. A compatibility route is available in
the existing project/file/Git/Job tools without adding new OAuth scopes.

Call list_projects **without** workspace on a parent configured with one
Workspace. It preserves local Projects and appends remote Projects that are
currently read-authorized by each paired child. The remote Project ID is a
synthetic legacy workspace selector:

    node:<24-hex-child-node-id>:<child-workspace-id>:<child-project-id>

Pass the **full ID** as workspace and omit project when using the cached tools:

    read_file({"workspace":"node:0123456789abcdef01234567:root:app","path":"src/main.rs"})
    git_status({"workspace":"node:0123456789abcdef01234567:root:app"})

The same selector works with inspect_project, list_directory, search_code,
git_diff, git_log, write_file, apply_patch, create_directory, run_command,
and git_commit. The parent does **not** mount the child Project locally:
each call traverses pinned TLS Transfer, and the child validates exact
Workspace/Project grants and its own current permissions. Unsupported
remote tools (notably run_shell, git_push and Docker) stay disabled.

Remote run_command returns a routable Job handle in job_id:

    nodejob:<child-node-id>:<child-workspace>:<child-project>:<child-job-id>

Pass this handle unchanged to cached get_job, get_job_output or cancel_job.
Only the child stores and runs the Job. Remote write_file, apply_patch,
create_directory and git_commit use deterministic request IDs derived from
the operation and complete argument snapshot. Remote commands keep the
caller's explicit request_id. Remote results expose remote_request_id for
reconciliation via the Nodes Dashboard. Mutations are **never replayed**
automatically after timeouts, and parent operation logs store only argument
digests and redacted remote output. If a result is uncertain, inspect the
child's request status and Git/filesystem state before retrying.

This is a compatibility path for already-cached tool schemas, **not** a
claim that ChatGPT's tool list has refreshed. Clients with the complete
tool catalog should continue to use explicit node_read/node_write.

## Phase 4 — Remote Mutations, Jobs and Git Checkpoints

The parent exposes `node_write` for explicitly supported remote operations: `write_file`, `apply_patch`, `create_directory`, `run_command`, `get_job`, `get_job_output`, `cancel_job`, and `git_commit`. The request contains `node_id`, `tool`, and `arguments` with exact child `workspace` and `project`. The child re-checks the pairing grant and its current local Project permissions, preserves local file hash and Git commit/diff concurrency checks, and stores Jobs in its own SQLite database. Remote shell, push, Docker and recursive forwarding are disabled. Parent MCP requires `files:write`, `commands:execute` and `git:write`; this is an intentional conservative scope combination. File-content mutation requests are represented by digests in parent/child operation logs.

A remote command returns its durable child-owned Job ID. Query it with `node_write` and `tool=get_job` or `get_job_output` using exact child workspace/project. Network timeout does not imply the Job failed, and **must not automatically replay mutations**. Use the original idempotency `request_id` if reconciling the same command.

## Phase 5 — Disconnection, Resume, Revocation and Recovery

Parent-side `node_read` also accepts `get_task_checkpoint`, `continue_task`, and `list_task_checkpoints` to inspect existing child-owned stage checkpoints. These must include the exact granted child `workspace` and `project`; list requests without both selectors are not accepted. Jobs stay on the child and are not re-executed when the parent reconnects. The parent can poll a previously received Job ID through `node_write` (`get_job` / `get_job_output`), and submitting the *same* `request_id` and identical command on the child returns the existing Job reference, not a duplicate. If a network timeout occurs before receiving a Job ID, do not invent a new request ID; use the original one only for idempotent reconciliation.

The child TLS identity, pairing token digest, Project grants and Job database survive Transfer listener restarts; parent reconnects using its pinned child certificate fingerprint and saved pairing token. Revoking the pair at the child immediately causes later calls to fail, even if the parent still has its local pairing entry. No write action is automatically retried after a connection error. The Transfer server sends bounded error responses for denied calls instead of silently dropping the connection. Paired node listings distinguish recent LAN discovery from trust: an undiscovered paired peer may still be reachable by its saved endpoint.

The local Nodes Dashboard now has a guarded `POST /api/nodes/write` path and an advanced Project-operation panel. It requires an explicit confirmation, exact selected Workspace/Project, and accepts only the same write whitelist as `node_write`. The parent records a request digest rather than file contents. Remote shell, arbitrary Docker tools, push and nested forwarding remain disabled.

## Phase 6 — Durable Request History and Safe Retention

The parent can use read-only Transfer operations with exact child Workspace/Project selectors:

- `request_status` with `request_id` returns the child-owned durable state, related Job ID/status when available, and a recovery recommendation. `completed` means the request was accepted and its response saved; for `run_command`, check `job_status` separately. An `interrupted` or `failed` mutation does **not** prove its side effects were rolled back.
- `request_history` accepts `limit` (1–50, default 20) and an optional opaque `cursor`. Follow `next_cursor` until it is null; the cursor is bound to the authenticated parent and Project and rejected outside that scope. Pages use immutable insertion ordering and include only bounded metadata, not file content, command output, raw args or saved result bodies. Records predating the history index still require exact-ID lookup.
- The local Nodes page offers Request status and Request history with a Load older requests control. Status badges distinguish a submitted command from its child Job's actual outcome.

Request IDs are permanent deduplication keys, not disposable log entries. To bound private SQLite growth without permitting replay, after **30 days** successful cached response bodies are converted into **permanent result-expired tombstones**. The fingerprint, node/Project scope, request ID, Job reference and non-replayable state remain. A duplicate request with an expired response is rejected, **never executed again**. Failed, interrupted and uncertain requests remain non-replayable and are not deleted. Compaction is limited to **200 responses per pass** on startup, periodic retention maintenance or explicit storage maintenance; it is not a destructive request-ID garbage collection. The storage metadata exposes the cache policy and number of durable request records.

Do not use a new request ID to "fix" an uncertain remote mutation without first checking the target Project, Git state or Job. A server restart does not automatically resume or replay unfinished filesystem/Git actions.

## Phase 7 — Indexed History, Local Metrics, and TCP Fault Verification

SQLite schema v4 adds the `idx_transfer_history_scope_created` partial expression index for
`request_history`, scoped by authenticated parent node, Workspace, Project, immutable
creation time and request key. Migrations preserve existing persisted requests. The
history cursor remains Project-scoped and never replaces per-request authorization.

The local-only Dashboard endpoint `GET /api/nodes/metrics` returns aggregate counts of
persisted requests by state, review-required requests, and recent `transfer_*` operation
outcomes over the previous 24 hours. The Nodes page shows total requests, completed
requests, review-required requests and the success rate for finished calls. This
endpoint is intentionally **not** routed on the public MCP server; results contain
no pairing tokens, request arguments, file data, or raw Job output.

Fault-injection tests open real pinned-TLS TCP connections: one connection stops
midway through a length-prefixed JSON request, and another sends a complete
authorized mutation then closes before reading its response. The first must not
create a request or side effect; the second must persist its result, allowing
a reconnect with the same request ID to return the cached result without
creating a second operation. These tests do not simulate arbitrary packet loss
or network latency; the fail-closed rule for uncertain mutations remains unchanged.

The TLS server tracks its accepted sessions and reaps completed tasks. On shutdown it
stops accepting new connections, gives existing sessions up to **3 seconds** to
finish and then aborts remaining half-open sessions before releasing the
listener's Runtime references. If a mutation is interrupted, its persisted
request identity remains non-replayable; operators must inspect the child state
rather than blindly submitting a new request ID.

## Planned authorization and routing invariants

- Pairing must use an encrypted authenticated protocol. The discovery name and IP are untrusted hints, not identities.
- Every remote request must be explicitly authorized by the child and constrained by the child's existing Project permissions; a remote parent cannot escalate local permissions.
- Remote mutations must retain optimistic concurrency controls (file SHA-256, Git diff digest/HEAD, unique Job request_id), and timeouts cannot silently replay work.
- The local Dashboard remains bound to loopback. Transfer must never expose the Dashboard or public OAuth owner credentials.
- The topology is a single parent with multiple children in the first release. No implicit recursive forwarding.
