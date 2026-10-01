# EndlessVibe

EndlessVibe 是一个使用 Rust 编写的 **self-hosted、single-owner MCP 开发后端**。它让 ChatGPT 等 MCP 客户端在你明确授权的本地项目中读取和修改文件、搜索代码、运行受限命令，并通过可审查的流程创建 Git commit。

项目默认采用保守安全策略：工作区必须显式登记，文件修改需要版本哈希，命令执行默认进入 bubblewrap 沙箱，网络与通用 Shell 默认关闭，MCP 工具调用通过 OAuth 授权。

当前版本：**v0.3.0**

## 特性

- **显式工作区授权**：只暴露配置中登记的项目，不扫描整个 HOME。
- **细粒度权限**：每个工作区可分别控制写入、命令执行和 Git commit。
- **OAuth for MCP**：支持 Authorization Code、S256 PKCE 和 Dynamic Client Registration。
- **冲突安全的文件编辑**：写入和 patch 使用 SHA-256 做乐观并发检查。
- **代码搜索**：在授权目录内执行有界文本/正则搜索，过滤常见敏感路径。
- **异步任务**：命令立即返回 job ID，可查询输出、状态、超时和取消。
- **默认沙箱执行**：Linux 下默认使用 bubblewrap，项目可写，系统工具目录只读，网络默认关闭。
- **受审查的 Git 提交**：先生成精确 diff review token，再只提交显式选择的文件。
- **内嵌状态页**：服务自身提供 Web 状态主页，不需要单独的 Node.js 前端。
- **单二进制部署**：核心服务由 Rust 构建，默认监听端口为 `20000`。

## 安全模型

EndlessVibe 面向 **单用户、自托管开发环境**，不是多租户托管平台。

默认情况下：

- 未授权客户端不能执行 MCP 工具。
- 工作区必须使用绝对路径显式登记。
- 文件工具不能越过工作区边界，也不会跟随符号链接。
- `run_shell` 默认关闭。
- 命令网络访问默认关闭。
- bubblewrap 不挂载整个 HOME、SSH 凭据、Docker socket 或服务状态目录。
- Git 提交只能针对经过 `git_diff` 审查的精确文件快照。
- 不提供 `git push`、`reset`、`clean` 或自动回滚。

如果启用 host execution，命令将拥有当前宿主机用户权限。只有在明确理解风险后才应使用：

```toml
[execution]
backend = "host"
acknowledge_unsafe_host_execution = true
```

更完整的边界与威胁模型见 [docs/SECURITY.md](docs/SECURITY.md) 和 [docs/EXECUTION.md](docs/EXECUTION.md)。

## 环境要求

推荐环境：

- Linux 5.6+（文件隔离依赖 `openat2`）
- Rust / Cargo 1.88+
- Git
- C 编译器（`rusqlite` 使用 bundled SQLite）
- bubblewrap（使用默认命令沙箱时）
- 可用的非特权用户命名空间

服务拒绝以 root 身份启动。

## 构建

```bash
cargo build --release
```

验证：

```bash
cargo test --all-targets
cargo build --release
```

## Quick Start

### 1. 初始化配置

使用真实存在的项目绝对路径：

```bash
./target/release/endlessvibe \
  --init \
  --workspace my-project=/absolute/path/to/project \
  --public-url https://mcp.example.com
```

初始化会创建配置和随机的 owner key，但不会修改工作区中的项目文件。

默认配置路径为 `$XDG_CONFIG_HOME/endlessvibe/config.toml`，未设置 `XDG_CONFIG_HOME` 时使用 `~/.config/endlessvibe/config.toml`；默认状态目录为 `$XDG_STATE_HOME/endlessvibe`，未设置时使用 `~/.local/state/endlessvibe`。

如果希望先以更保守的方式试运行：

```bash
./target/release/endlessvibe \
  --init \
  --workspace my-project=/absolute/path/to/project \
  --public-url https://mcp.example.com \
  --read-only \
  --no-exec
```

通用配置示例见 [config/config.example.toml](config/config.example.toml)。

### 2. 向已有配置添加项目

先停止正在运行的 EndlessVibe，再执行：

```bash
./target/release/endlessvibe --add-project my-project=/absolute/path/to/project
```

也可以省略 ID，此时使用目标目录名作为 workspace ID：

```bash
./target/release/endlessvibe --add-project /absolute/path/to/project
```

`--add-project` 可以重复使用，并可搭配 `--read-only` 或 `--no-exec`。修改采用临时文件 + 原子替换；若 ID 重复、路径重复、目录不存在或配置校验失败，则不会写入配置。完成后重新启动服务加载新的 workspace。

父 workspace 与其子项目可以同时登记，并共享同一把操作锁，避免文件/Git 操作通过嵌套 workspace 并发绕过锁；两个 ID 指向同一目录仍会被拒绝。

### 3. 启动服务

```bash
./target/release/endlessvibe
```

默认本地状态页：`http://127.0.0.1:20000/`

默认本地 MCP endpoint：`http://127.0.0.1:20000/mcp`

对公网提供服务时，应使用 HTTPS 反向代理，并让 `public_url` 与实际公开 origin 保持一致。

### 4. 连接 ChatGPT

在支持 MCP OAuth 的客户端中：

- Name：`EndlessVibe`
- MCP URL：`https://mcp.example.com/mcp`
- Authentication：**OAuth**
- Client registration：优先使用自动注册 / DCR

不要把 `owner.key` 放入 MCP URL、聊天记录或公开配置。授权时只在你自己的 EndlessVibe 授权页面中输入 owner key。

EndlessVibe 会公开 MCP/OAuth 所需的发现元数据，但实际工具执行仍需要有效授权。OAuth 行为、回调 URI 和排错说明见 [docs/OAUTH.md](docs/OAUTH.md)。

## Cloudflare Tunnel / Reverse Proxy

如果 EndlessVibe 运行在宿主机，而 cloudflared 运行在 Docker 中，可以把整个公开 hostname 转发到：

```text
http://host.docker.internal:20000
```

不要只代理 `/mcp`，OAuth 还需要 `/.well-known/*` 和 `/oauth/*`。无需把项目目录或 Docker socket 挂载给 cloudflared。

示例与排错见 [docs/DOCKER_TUNNEL.md](docs/DOCKER_TUNNEL.md)。

## MCP 工具

| 类别 | 工具 |
|---|---|
| 连接 | `hello`, `get_service_status` |
| 项目 | `list_projects`, `inspect_project` |
| 文件 | `list_directory`, `read_file`, `write_file`, `apply_patch`, `create_directory`, `search_code` |
| 命令 | `run_command`, `run_shell` |
| 任务 | `get_job`, `get_job_output`, `cancel_job`, `list_jobs` |
| Git | `git_status`, `git_diff`, `git_log`, `git_commit` |

### 文件编辑

现有文件写入必须携带 `read_file` 返回的 `expected_sha256`。创建新文件时使用字面量 `MISSING`。

`apply_patch` 使用精确的 `old_text -> new_text` 替换，不是 unified diff，也不会模糊匹配。

### 异步命令

`run_command` 和 `run_shell` 返回持久化的 `job_id`，之后通过任务工具查询状态和输出。

相同工作区中，相同 `request_id` + 相同请求参数会在去重窗口内复用；使用相同 ID 但参数不同会返回冲突。服务重启后，之前仍在运行的任务会标记为 `interrupted`，不会自动重放可能具有副作用的命令。

### Git 提交

推荐流程：

```text
git_status
   ↓
git_diff(paths)
   ↓
审查 diff
   ↓
git_commit(paths, expected_head, expected_diff_sha256)
```

`git_commit` 只提交显式指定的文件，拒绝 selected paths 中已有的用户 staged 内容，保留其他无关 staged 内容，并使用 HEAD + diff hash 防止审查后的内容被替换。它不会执行 hooks、签名或 push。

恢复和限制见 [docs/GIT_RECOVERY.md](docs/GIT_RECOVERY.md)。

## 配置与执行边界

每次 MCP 调用都显式携带 workspace ID 和相对路径。EndlessVibe 没有全局“当前项目”状态。

默认 bubblewrap 模式只开放当前授权项目、独立构建缓存和必要的只读系统程序目录。自定义 Rustup、SDK 或工具链可以通过只读挂载和 PATH 显式加入沙箱。首次下载依赖需要预填缓存，或由用户明确允许沙箱网络。

详细说明见 [docs/EXECUTION.md](docs/EXECUTION.md)。

## 已知限制

当前 Git 工具只支持普通 standalone repository（`.git` 为目录）。父/子 workspace 可以共存并共享操作锁，但同一路径不能通过多个 ID 重复授权。linked worktree、bare repository、某些 submodule 提交流程、external object alternates、partial clone、sparse 配置以及依赖 Git filters/LFS helper 的转换流程会被拒绝或不受支持。

文件工具会过滤常见敏感文件名，但无法判断所有普通源码文件中是否包含秘密信息。不要把凭据放入允许执行命令的工作区。

备份、任务输出和 SQLite 状态属于私有数据，需要由部署者自行维护保留和清理策略。

## 项目结构

```text
src/
├── config.rs
├── runtime.rs
├── server.rs
├── mcp.rs
├── workspace.rs
├── store.rs
├── web.rs
├── util.rs
├── security/
│   ├── auth.rs
│   └── paths.rs
└── tools/
    ├── filesystem.rs
    ├── process.rs
    ├── jobs.rs
    ├── git.rs
    └── types.rs

web/                  内嵌状态页面
config/               配置示例
deploy/               Cloudflare Tunnel 示例
tests/integration.rs   Rust 集成测试
docs/                  安全、OAuth、工具、沙箱与恢复文档
```

## 文档

- [OAuth 与 ChatGPT 接入](docs/OAUTH.md)
- [MCP 工具参数](docs/TOOLS.md)
- [命令执行与 sandbox](docs/EXECUTION.md)
- [安全模型](docs/SECURITY.md)
- [Git 恢复与限制](docs/GIT_RECOVERY.md)
- [Cloudflare Tunnel](docs/DOCKER_TUNNEL.md)

## v0.2 → v0.3

v0.3 引入了强制 OAuth、显式工作区、写入/执行权限、异步任务、受限文件编辑与 Git commit。

旧版 No Authentication 连接不能直接访问 v0.3 的私有工具。升级后需要重新初始化配置，并在 MCP 客户端中使用 OAuth 重新授权。

## License

[MIT](LICENSE)
