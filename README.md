# EndlessVibe

EndlessVibe 是一个使用 Rust 编写的 **self-hosted、single-owner MCP 开发后端**。它让 ChatGPT 等 MCP 客户端在明确授权的本地 Project 中读取和修改文件、搜索代码、运行受限命令，并通过可审查流程创建 Git commit。

当前版本：**v0.3.2**

## Workspace 与 Project

EndlessVibe 使用两层模型：

```text
Workspace: projects -> /home/user/projects
├── Project: BAfter       -> BAfter/
├── Project: EndlessVibe  -> EndlessVibe/
└── Project: mHypr        -> mHypr/
```

- **Workspace** 是管理根节点，只定义允许挂载 Project 的文件系统边界。
- **Project** 是实际操作单元，拥有独立的写入、命令执行、Git commit 权限与操作锁。
- Project 的配置路径必须是 Workspace 根目录下的相对路径，不能使用 `..` 或逃离根目录。
- 不同 Project 使用独立锁，可以并行执行；同一 Project 的文件、命令和 Git 操作会串行化。
- 命令任务的全局并行数仍由 `limits.max_jobs` 控制。

因此 MCP 客户端应先调用 `list_workspaces`，再调用 `list_projects(workspace)`，后续项目操作显式携带 `workspace + project`。

## 特性

- **两层授权模型**：Workspace 管理根边界，Project 管理实际权限。
- **Project 级并发**：不同 Project 可并行，同一 Project 保持互斥。
- **OAuth for MCP**：Authorization Code、S256 PKCE、Dynamic Client Registration。
- **冲突安全编辑**：文件写入和 patch 使用 SHA-256 做乐观并发检查。
- **异步任务**：命令立即返回 Job ID，可查询状态、输出、超时和取消。
- **长任务 Checkpoint**：`task_id + stage` 将 Job 与 Git commit 关联；成功 commit 自动成为阶段恢复点，stream 中断后可直接查询恢复。
- **默认 Bubblewrap 沙箱**：网络与通用 Shell 默认关闭。
- **受审查 Git 提交**：`git_diff` 生成 review token，`git_commit` 只提交明确审查过的文件。
- **内嵌状态页**：无需独立前端服务。

## 安全模型

EndlessVibe 面向单用户、自托管开发环境，不是多租户托管平台。默认情况下，未授权客户端不能执行私有 MCP 工具；Workspace 必须显式登记；Project 必须位于对应 Workspace 根内；文件工具不能越过 Project 根目录，也不会跟随符号链接；`run_shell` 和沙箱网络默认关闭；Bubblewrap 不挂载整个 HOME、SSH 凭据、Docker socket 或服务状态目录；Git 工具默认不允许 push，并且不提供 reset、clean 或自动回滚；只有 Project 显式启用 `allow_git_push` 后，专用 `git_push` 才可执行受限的非 force 推送。

如果显式启用 host execution，命令将拥有当前宿主机用户权限：

```toml
[execution]
backend = "host"
acknowledge_unsafe_host_execution = true
```

完整边界见 [docs/SECURITY.md](docs/SECURITY.md) 和 [docs/EXECUTION.md](docs/EXECUTION.md)。

## 环境要求

- Linux 5.6+
- Rust / Cargo 1.88+
- Git
- C 编译器（`rusqlite` 使用 bundled SQLite）
- bubblewrap（默认执行后端）
- 可用的非特权 user namespace

服务拒绝以 root 身份启动。

## 构建与验证

```bash
cargo test --all-targets
cargo build --release
```

Bubblewrap 基础环境可以单独检查：

```bash
./target/release/endlessvibe --check-sandbox
```

## Quick Start

### 1. 创建 Workspace 根

```bash
./target/release/endlessvibe \
  --init \
  --workspace projects=/home/user/projects \
  --public-url https://mcp.example.com
```

`--init` 创建 Workspace 根、配置文件和随机 owner key。Workspace 本身没有写入/执行/Git 权限。

默认配置路径为 `$XDG_CONFIG_HOME/endlessvibe/config.toml`，未设置时为 `~/.config/endlessvibe/config.toml`；状态目录默认是 `$XDG_STATE_HOME/endlessvibe`，未设置时为 `~/.local/state/endlessvibe`。

### 2. 挂载 Project

先停止运行中的 EndlessVibe，再执行：

```bash
./target/release/endlessvibe \
  --add-project projects:EndlessVibe=/home/user/projects/EndlessVibe
```

如果省略 `:PROJECT_ID`，会使用目标目录名：

```bash
./target/release/endlessvibe \
  --add-project projects=/home/user/projects/BAfter
```

可以重复 `--add-project` 一次挂载多个 Project。`--read-only` 会关闭新 Project 的写入/执行/提交权限；`--no-exec` 只关闭命令执行。目标目录必须已经存在，并且必须严格位于指定 Workspace 根目录下。

生成的配置结构类似：

```toml
[[workspaces]]
id = "projects"
path = "/home/user/projects"

[[workspaces.projects]]
id = "EndlessVibe"
path = "EndlessVibe"
allow_write = true
allow_exec = true
allow_git_commit = true
allow_git_mutation = false
allow_git_push = false
```

旧版 flat workspace 配置仍可读取用于迁移，但新 CLI 只写入两层格式。

### 3. 启动

```bash
./target/release/endlessvibe
```

本地 Dashboard：`http://127.0.0.1:20001/`

MCP endpoint：`http://127.0.0.1:20000/mcp`

公网部署应使用 HTTPS，并让 `server.public_url` 与公开 origin 一致。

### 4. 进程控制

新版本启动后会在私有 state 目录维护 `service.pid`，记录 PID 与 Linux `/proc` start time，避免 PID 被复用后误杀其他进程。

```bash
# 查看状态
./target/release/endlessvibe --status

# 优雅退出
./target/release/endlessvibe --stop

# 优雅停止旧实例，然后由当前二进制直接重新启动
./target/release/endlessvibe --restart
```

如果使用非默认配置：

```bash
./target/release/endlessvibe --config /path/to/config.toml --restart
```

`--stop` / `--restart` 发送 `SIGTERM` 并最多等待 15 秒，不会自动 `SIGKILL`。如果升级前的旧实例还没有 `service.pid`，需要最后一次使用原有方式停止并启动新版本；从新版本成功启动后即可一直使用上述内置命令。

### 5. 连接 ChatGPT

- Name：`EndlessVibe`
- MCP URL：`https://mcp.example.com/mcp`
- Authentication：OAuth
- Client registration：自动注册 / DCR

不要把 `owner.key` 放入 URL、聊天记录或公开配置。详见 [docs/OAUTH.md](docs/OAUTH.md)。

## MCP 工具

`hello` / `get_service_status` 会同时返回服务版本、`tool_schema_revision` 和工具数量。若 Dashboard `/mcp` 已显示新的 revision，但 ChatGPT 仍缺少新工具，说明客户端仍缓存旧 MCP schema，需要重新连接或刷新插件。

| 类别 | 工具 |
|---|---|
| 连接 | `hello`, `get_service_status` |
| Workspace / Project | `list_workspaces`, `list_projects`, `inspect_project` |
| 文件 | `list_directory`, `read_file`, `write_file`, `apply_patch`, `create_directory`, `search_code` |
| 命令 | `run_command`, `run_shell` |
| 任务 | `get_job`, `get_job_output`, `cancel_job`, `list_jobs`, `get_task_checkpoint`, `list_task_checkpoints` |
| Git | `git_status`, `git_diff`, `git_log`, `git_commit`, `git_push` |

除连接和 Job 查询类工具外，Project 操作统一使用：

```json
{"workspace":"projects","project":"EndlessVibe"}
```

路径始终相对于 Project 根，而不是 Workspace 根。

### 文件编辑

`read_file` 返回完整文件 SHA-256。修改已有文件必须把该摘要作为 `expected_sha256`；创建新文件使用字面量 `MISSING`。`apply_patch` 是精确 `old_text -> new_text` 替换，不是 unified diff。

### 异步命令与并发

`run_command` / `run_shell` 的请求键由 `workspace + project + request_id` 组成。相同组合和相同参数在去重窗口内复用；参数不同则冲突。

例如 `projects/BAfter` 与 `projects/mHypr` 可以同时执行任务，只要没有超过全局 `max_jobs`；`projects/BAfter` 自己的文件、命令和 Git 操作会争用同一 Project 锁。

长任务建议始终携带稳定的 `task_id` 和当前 `stage`：

```json
{"workspace":"projects","project":"BAfter","program":"cargo","args":["check"],"request_id":"release-check-01","task_id":"release-031","stage":"validate","timeout_seconds":120}
```

Job 会把 `job_id` 和终态写入阶段 checkpoint。阶段最终通过 `git_commit` 提交时继续传相同的 `task_id + stage`，该 commit SHA 会成为该阶段的持久恢复点。ChatGPT stream 中断后使用：

```json
{"workspace":"projects","project":"BAfter","task_id":"release-031"}
```

调用 `get_task_checkpoint`，即可取得最新阶段、关联 Job、Job 状态和最后 commit，无需猜测上一轮执行到了哪里。Job 成功本身不代表阶段完成；只有成功 commit 的阶段状态为 `committed`。

### Git

推荐流程：

```text
git_status(workspace, project)
  → git_diff(workspace, project, paths)
  → 审查 diff
  → git_commit(workspace, project, paths, expected_head, expected_diff_sha256)
```

`git_commit` 不执行 hooks、签名或 push，不覆盖无关暂存内容，也不改写工作树。长任务可以额外传 `task_id + stage`；commit 成功后会自动记录 checkpoint。需要在 bubblewrap 内通过 `run_command git` 执行 `switch/merge/branch/add/commit` 等本地 Git 变更时，可对单个 Project 显式设置 `allow_git_mutation=true`；它要求 `allow_write=true` 和 `allow_exec=true`。远端推送使用独立的 `allow_git_push=true` 与 `git_push(remote, branch, expected_head)`；它先校验本地 branch HEAD、查询远端 HEAD 并执行 dry-run，只允许把已存在的本地 branch 非 force 地推送到预配置 remote 的同名 branch。`run_command` 仍继续拒绝 `git push/fetch/pull/...` 网络子命令。详见 [docs/GIT_RECOVERY.md](docs/GIT_RECOVERY.md) 与 [docs/EXECUTION.md](docs/EXECUTION.md)。

## Cloudflare Tunnel

如果 EndlessVibe 在宿主机、cloudflared 在 Docker 中，可以将整个 hostname 转发到 `http://host.docker.internal:20000`。不要只代理 `/mcp`，OAuth 还需要 `/.well-known/*` 和 `/oauth/*`。详见 [docs/DOCKER_TUNNEL.md](docs/DOCKER_TUNNEL.md)。

## 已知限制

Git 工具只支持 `.git` 为真实目录的 standalone repository，不支持 linked worktree、bare repository、部分 clone/sparse 配置和依赖 filters/LFS helper 的转换流程。Project 之间禁止目录重叠；一个 Project 只能属于一个明确的 Workspace 根。

文件工具会过滤常见敏感名称，但不能识别所有源码中的凭据。允许执行命令的 Project 中不应保存真实秘密。

## 项目结构

```text
src/
├── config.rs
├── runtime.rs
├── workspace.rs
├── mcp.rs
├── security/
└── tools/
    ├── filesystem.rs
    ├── process.rs
    ├── jobs.rs
    ├── git.rs
    └── types.rs

config/               配置示例
deploy/               Cloudflare Tunnel 示例
tests/integration.rs   Rust 集成测试
docs/                  安全、OAuth、工具、沙箱与恢复文档
web/                   内嵌状态页面
```

## 文档

- [MCP 工具参数](docs/TOOLS.md)
- [命令执行与 sandbox](docs/EXECUTION.md)
- [安全模型](docs/SECURITY.md)
- [Git 恢复与限制](docs/GIT_RECOVERY.md)
- [OAuth 与 ChatGPT 接入](docs/OAUTH.md)
- [Cloudflare Tunnel](docs/DOCKER_TUNNEL.md)

## License

[MIT](LICENSE)
