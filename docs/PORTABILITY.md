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
| Linux built-in `--stop` / `--restart` | Supported | Use OS process manager | Use OS process manager |
| Docker Engine Unix-socket integration | Optional | Depends on Unix socket | Unix-socket transport unavailable |

**Important:** a successful Linux build is *not* proof that either other
operating system builds successfully. Native builds and unit tests must be
run on Windows and macOS before a release is described as supporting them.

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
