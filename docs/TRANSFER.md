# EndlessVibe Transfer / Parent–Child Nodes

## Phase 1 — LAN Discovery

`[transfer]` is disabled by default. Enabling it starts an independent UDP multicast discovery service on `239.255.77.77:20003`, announcing only the stable node ID, display name and declared transfer port. These packets are **not authenticated**; discovery results are **never authorization**. The node ID is generated once and persisted in the private SQLite store. Offline peers are identified by stale `last_seen` timestamps. A local-only `GET /api/nodes` endpoint exposes the discovered nodes. Cross-subnet/isolated-Wi-Fi installations require a later manual-address option. The actual encrypted transfer listener is added in Phase 2; announcing a port does not make a peer trusted.

```toml
[transfer]
enabled = false
listen = "0.0.0.0:20002"
advertise = true
discover = true
display_name = "EndlessVibe"
```

## Planned authorization and routing invariants

- Pairing must use an encrypted authenticated protocol. The discovery name and IP are untrusted hints, not identities.
- Every remote request must be explicitly authorized by the child and constrained by the child's existing Project permissions; a remote parent cannot escalate local permissions.
- Remote mutations must retain optimistic concurrency controls (file SHA-256, Git diff digest/HEAD, unique Job request_id), and timeouts cannot silently replay work.
- The local Dashboard remains bound to loopback. Transfer must never expose the Dashboard or public OAuth owner credentials.
- The topology is a single parent with multiple children in the first release. No implicit recursive forwarding.
