<p align="center">
  <img src="EndlessVibe.png" alt="EndlessVibe logo" width="156">
</p>

<p align="center"><strong>简体中文</strong> · <a href="README.en.md">English</a></p>

<h1 align="center">EndlessVibe</h1>

<p align="center">
  <strong>把本地开发环境安全地连接到 AI 助手。</strong>
  <br>
  一个使用 Rust 构建的自托管 MCP 开发服务，统一管理项目、代码、命令、Git 与远程节点。
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.3.5-2563eb" alt="Version 0.3.5">
  <img src="https://img.shields.io/badge/Rust-1.88%2B-de6a36?logo=rust&logoColor=white" alt="Rust 1.88+">
  <img src="https://img.shields.io/badge/platform-Linux-34495e?logo=linux&logoColor=white" alt="Linux">
  <img src="https://img.shields.io/badge/MCP-Streamable%20HTTP-7c3aed" alt="MCP Streamable HTTP">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-22a06b" alt="MIT License"></a>
</p>

<p align="center">
  <a href="#screenshots">界面预览</a> ·
  <a href="#features">核心功能</a> ·
  <a href="#quick-start">快速开始</a> ·
  <a href="#architecture">架构</a> ·
  <a href="#security">安全</a> ·
  <a href="#documentation">文档</a>
</p>

---

## Overview

**EndlessVibe** 让 ChatGPT 等支持 MCP（Model Context Protocol）的客户端通过统一接口操作**已授权**的本地代码项目，并提供可在浏览器中查看的 Dashboard。

它不是云端 IDE，也不是多用户远程 Shell。服务由使用者自行托管，核心设计是**显式授权、按 Project 隔离、操作可审查、长任务可恢复**。

- **MCP Server**：通过 Streamable HTTP 和 OAuth 2.0 授权客户端。
- **Local Dashboard**：查看项目、任务、代码操作、执行记录与节点状态。
- **Project Runtime**：提供文件、命令、Git、Checkpoint 及持久化 Job 管理。
- **Node Transfer（可选）**：在局域网中配对其他 EndlessVibe 节点，远程访问获授权的 Project。

## Screenshots

### Dashboard

![EndlessVibe Dashboard](docs/screenshots/dashboard.jpg)

<sub>深色主题 · 简体中文 — 服务状态、执行指标与本地控制台。</sub>

| Projects | Tasks |
| :---: | :---: |
| ![Project management](docs/screenshots/projects.jpg) | ![Task management](docs/screenshots/tasks.jpg) |
| Workspace / Project 权限与配置 | Job 状态、阶段与 Checkpoint |

| Operations | Nodes |
| :---: | :---: |
| ![Operation history](docs/screenshots/operations.jpg) | ![Node management](docs/screenshots/nodes.jpg) |
| 操作审计与执行记录 | 节点发现、配对与远程请求诊断 |

<sub>截图使用当前工作区新版前端与本地运行服务的数据生成，已脱敏部分本机标识。英文版 README 展示亮色英文界面；Nodes / Transfer 功能默认关闭，需要主动配置。</sub>

## Features

| 模块 | 能力 |
| --- | --- |
| **Workspace & Project** | 两层工作空间模型，项目级读写、执行和 Git 权限，独立操作锁 |
| **Files & Search** | 安全列目录、读取、创建、写入、精确 Patch、代码搜索；SHA-256 冲突检查 |
| **Commands & Jobs** | 异步命令、持久 Job ID、输出分页、超时、取消、请求去重 |
| **Git Workflow** | Status、Diff 审查、Log、Commit；按需启用本地 Git 修改或受限 Push |
| **Tasks & Recovery** | `task_id + stage` Checkpoint、Job 关联、提交恢复点、重启诊断 |
| **Dashboard** | 首次运行 Setup、ChatGPT 连接帮助、状态图表、Projects、Tasks、Operations、Nodes、配置与存储管理 |
| **Authentication** | OAuth 2.0 Authorization Code、PKCE、动态客户端注册及工具 Scope |
| **Node Transfer** | 局域网发现、TLS 配对、Project 授权、远程请求状态与历史 |
| **Execution Profiles** | Linux Bubblewrap、Windows AppContainer 离线沙箱；显式项目授权 |
| **Docker（可选）** | 通过受限 Engine API 查看、管理获授权的容器 |

### Workspace 与 Project

`Workspace` 只是根目录管理边界，`Project` 才是可执行操作的授权单元：

```text
Workspace: projects                  /home/user/projects
├── Project: website                 website/
├── Project: kernel                  kernel/
└── Project: tools                   tools/
```

不同 Project 可以并行工作；同一个 Project 的文件、命令和 Git 修改共享操作锁，避免互相覆盖。

## Quick Start

### Requirements

- **Linux 5.6+、macOS、Windows 10/11**（macOS / Windows 为新增适配平台，需要在对应系统验证后正式发布）
- Rust / Cargo **1.88+**
- Git、C/C++ 编译工具链（需安装于目标操作系统）
- Linux 默认需要 Bubblewrap 与可用的非特权 User Namespace；Windows 可选择原生 `appcontainer` 离线沙箱。macOS / Windows 新配置仍默认禁用命令执行，`host` 需显式风险确认

Linux 下仍拒绝以 root 身份启动。跨平台文件操作采用目录句柄边界检查；**Bubblewrap 只在 Linux 可用，Windows 没有 Docker Unix Socket 接口，不能将两者视为跨平台已实现的功能**。详见 [跨平台支持与安全限制](docs/PORTABILITY.md)。

### 1. Build

在项目源码目录中：

```bash
cargo build --release
cargo test --locked
```

### 2. Start and open Setup

全新平台无需预先填写域名或 Workspace，直接启动程序：

```bash
./target/release/endlessvibe
```

Windows 对应命令为 `.\target\release\endlessvibe.exe`。将程序安装到 PATH 后，也可直接运行 `endlessvibe`。

配置不存在时，程序自动创建私有配置和 Owner Key，然后提供本机 [初始化与连接帮助](http://127.0.0.1:20001/setup)。初始配置没有任何 Workspace 或 Project，Node Transfer 默认关闭，命令执行沿用所在平台的安全默认值。

| 入口 | 网页首次初始化的默认值 |
| --- | --- |
| **Setup / 连接帮助** | [http://127.0.0.1:20001/setup](http://127.0.0.1:20001/setup)，仅本机 |
| **Dashboard** | [http://127.0.0.1:20001](http://127.0.0.1:20001)，仅本机 |
| **MCP** | `127.0.0.1:20000`，本机资源地址 `http://127.0.0.1:20000/mcp` |
| **Workspace / Project** | 均为空，添加前没有项目可供客户端操作 |
| **Nodes / Transfer** | 关闭，后续可按需要配置与配对 |

已有安装正常启动时沿用原配置，不会切换监听地址、删除项目或重置密钥。

### 3. Configure from the browser

在 `/setup` 中完成以下配置：

1. **添加 Workspace 管理根。** 填写简单 ID 和已经存在的绝对目录，例如 `projects` 与 `/home/user/projects`，或 Windows 的 `D:\project`。登记管理根不会自动授权其中的项目；随后前往 **Projects** 页面选择具体项目及其读写、执行、Git 权限。
2. **设置对外 HTTPS 域名。** 填写实际分配给你或由你管理的域名；例如将 `your-domain.ngrok.app` 或 `https://your-domain.ngrok.app` 替换为自己的 ngrok 域名。裸域名会补全为 HTTPS；此处填写域名或 origin，不附加 `/mcp`、密钥或查询参数。页面根据配置给出 MCP 地址和连接步骤。
3. **配置 Tunnel 或反向代理。** 将该 HTTPS 域名转发到服务实际的 MCP 监听端口，并保留 MCP 与 OAuth 端点的路由。域名设置写入 EndlessVibe 配置；DNS、证书或 Tunnel 仍由所使用的平台管理。

例如，在本机安装并配置 ngrok 后，可使用账户中可用的域名：

```bash
ngrok http 20000 --url https://your-domain.ngrok.app
```

把示例域名替换为实际分配或配置的域名，并在 Setup 中保存相同的 HTTPS origin；把 `20000` 替换为当前 MCP 监听端口。`20001` 是本机管理 Dashboard，保持仅本机访问。ngrok 参数详见 [官方 CLI 文档](https://ngrok.com/docs/gateway/agent/cli)；Cloudflare 部署见 [Tunnel 配置](docs/DOCKER_TUNNEL.md)。

### 4. Save and apply configuration

服务地址等配置保存后，页面会区分**已保存配置**与**当前运行状态**，提示是否需要重启。按原有启动方式重启：手动启动的实例使用终端或已有进程控制；systemd、Windows 服务或其他服务管理器管理的实例使用原管理器。网页提供操作提示，保存本身不会重启进程。

`ENDLESSVIBE_BIND`、`ENDLESSVIBE_PUBLIC_URL` 环境变量，以及启动时的 `--bind`、`--public-url` 参数可覆盖配置文件。若重启后仍显示旧地址，应同时检查原服务的环境变量和启动参数。重启后返回 Setup，核对实际生效地址，再继续连接客户端。

Linux / Windows 可通过 `--status`、`--stop`、`--restart` 管理已配置的服务实例。Windows 使用进程启动时间校验和专用退出事件优雅停止；从旧版升级时，须先用 Ctrl+C 或原服务管理器停止旧进程一次。macOS 使用原启动方式停止并启动。这些操作不需要开启 `host` 执行后端。

### 5. Connect ChatGPT

确认 HTTPS 入口和 OAuth 端点已可用后，在 ChatGPT 中操作：

1. 打开 [ChatGPT Plugins](https://chatgpt.com/plugins)，点击 **+ → Add custom MCP server**。
2. 填写名称与说明，输入完整的 `https://你的域名/mcp`，选择 OAuth。EndlessVibe 使用现有的动态客户端注册（DCR）。
3. 按页面提示确认权限，选择 **Create as a plugin**，检查发现的工具并安装。
4. 需要登录时，在你本机浏览器中的 **EndlessVibe OAuth 授权页面**输入 Owner Key 并确认授权。这个流程不需要 OpenAI API Key。
5. 新建会话，输入 `@` 选择 EndlessVibe，再尝试列出已授权项目。

**保存域名或本机配置成功不代表 ChatGPT 已经连接。** 需要完成插件安装、OAuth 授权和实际工具调用；Setup 不会把本机配置检查显示为公网连通性验证。Owner Key、访问令牌和登录凭据不要填写到 MCP URL 或聊天消息中。

界面入口及账户策略以 [OpenAI 官方接入说明](https://developers.openai.com/plugins/deploy/connect-chatgpt) 和 [OAuth 说明](https://developers.openai.com/plugins/build/auth) 为准。其他 MCP 客户端使用相同的 HTTPS `/mcp` 地址与 OAuth；服务端细节见 [OAuth 接入](docs/OAUTH.md)。

### Reopen Setup

可执行文件已加入 PATH 时，可随时运行：

```bash
endlessvibe --init
```

Windows 从项目源码根目录运行时，可直接使用以下命令，无需先添加 PATH：

```powershell
.\target\release\endlessvibe.exe --init
```

如使用自定义配置，可加 `--config`。Windows 示例（将路径替换为实际配置文件）：

```powershell
.\target\release\endlessvibe.exe --init --config "C:\path\to\config.toml"
```

如果服务正在运行，命令输出已有 `/setup` 地址后退出；如果服务已停止，则使用原配置启动。已有配置、Owner Key、Workspace / Project 授权和注释都会保留。`--init` 重新打开引导，不会清空安装数据。

### Optional: explicit CLI initialization

尚未创建配置时，也可保留原来的命令行初始化流程。以下 `--init --workspace ...` 会创建配置后退出，已有配置不会被覆盖：

```bash
mkdir -p "$HOME/projects"

./target/release/endlessvibe \
  --init \
  --workspace "projects=$HOME/projects" \
  --public-url https://mcp.example.com

# 目标项目目录须已存在；--add-project 要求服务停止
./target/release/endlessvibe \
  --add-project "projects:website=$HOME/projects/website"

./target/release/endlessvibe
```

Project 必须位于登记的 Workspace 根目录内。添加时可用 `--read-only` 或 `--no-exec` 收紧权限。Linux 已有配置可在需要时运行 `./target/release/endlessvibe --check-sandbox` 检查 Bubblewrap 和工具链；诊断、凭据维护及进程管理命令不会自动创建缺失配置。

## 局域网模式（无需公网 ChatGPT URL）

仅在局域网连接多台 EndlessVibe 时，可使用 **`--lan-only`**。该模式启用独立的 **TLS 1.3 Transfer 节点通信**，不会将未加密的 MCP/OAuth 直接暴露到局域网。

**Windows 新安装**（工作空间目录必须已存在）：

```powershell
.\target\debug\endlessvibe.exe --init --lan-only --workspace "projects=C:\path\to\projects"
```

**Windows 已有配置的安装**（包括首次网页初始化，不删除配置或 owner.key）：

```powershell
# 先停止正在运行的 EndlessVibe，然后只执行一次转换
.\target\debug\endlessvibe.exe --lan-only

# 正常启动：从安装目录运行，避免锁定 Cargo 输出的 EXE
.\deploy\windows\EndlessVibe.ps1 -Action Update
```

**Windows 开发时 `cargo build` 报 `Access is denied (os error 5)`：** Windows 无法覆盖正在运行的 `target\debug\endlessvibe.exe`，即使重启电脑后继续运行同一个 EXE 也会再次锁定。请在 Windows PowerShell 的项目根目录执行一次：

```powershell
.\deploy\windows\EndlessVibe.ps1 -Action Update
```

脚本在 `target\windows-stage` 构建，把运行程序安装至 `%LOCALAPPDATA%\Programs\EndlessVibe\releases\<SHA256>\endlessvibe.exe`，通过经过进程身份校验的 `--stop` 优雅停止旧进程，再启动新的安装副本；不会强制结束进程，也不更改 `config.toml`、Owner Key 或 Project 授权。脚本现已将安装清单的原子替换提前到停服之前，并使用非空备份路径；如果新服务启动失败，将尝试恢复旧清单与旧实例。之后普通 `cargo build`（默认 `target\debug`）不再与正在运行的 EXE 冲突。更新与启动命令为：

```powershell
.\deploy\windows\EndlessVibe.ps1 -Action Update   # 构建、安装并重启服务
.\deploy\windows\EndlessVibe.ps1 -Action Start    # 从已安装版本启动（不构建）
.\deploy\windows\EndlessVibe.ps1 -Action Status   # 查看运行状态及路径
.\deploy\windows\EndlessVibe.ps1 -Action Stop     # 优雅停止
```

使用非默认配置时，首次更新附加 `-ConfigPath 'C:\path\to\config.toml'`（必须是绝对路径），后续将沿用该路径。如使用任务计划程序、NSSM 或 Windows 服务自动启动，**也要将启动入口改为上述脚本的 `-Action Start`**，不能再次从 `target\debug` 运行。旧版本无法处理 `--stop` 或服务位于其他登录会话时，应先用原服务管理器停止；脚本不会强制结束任何进程。

LAN-only 会持久化到 `config.toml`，并保留原 Workspace / Project 权限与 Owner Key。原公网地址签发的 OAuth Token 不再对新的本机资源有效。

Windows 服务自更新现在提供 `windows_service_update_plan`、`windows_service_update` 与 `windows_service_update_status` 三个 MCP 工具，**默认关闭**；管理员必须显式绑定一个精确 Project。实际更新须 `confirm=true`、当前及候选版本 SHA-256 和唯一 request_id，由原生辅助进程完成优雅停服、更新与失败回滚，不启用 PowerShell。第一次仍需手动安装新版 Windows 子节点，并更新 Linux 父节点的 MCP 工具版本；完整安全边界见 [Windows 原生部署](docs/WINDOWS_DEPLOY.md)。

### Windows 原生命令沙箱

在已识别内核版本的 Windows 10/11 x64 上，通过本机 Dashboard **沙箱 → 执行后端** 选择 `appcontainer` 并重启服务，即可让已授权的 `run_command` Jobs 在 Windows LPAC 中运行。支持本机工具、Cargo 编译与测试、实时 stdout/stderr、任务摘要、取消和超时；每次任务使用独立身份与 Job Object，整个进程树退出后才撤销项目权限和回收私有目录。页面区分“已保存”与“当前运行”的后端，现有安装不会自动切换。异常退出后，若可信执行记录仍是本次 Windows 启动中的 `running`，恢复必须等待完整 Windows 重启并核验启动序号变化；`prepared` / `empty` 记录可在服务重启时恢复。缺失、损坏或不可信的证据不会因为重启 Windows 而变得有效，恢复仍会被拒绝。

项目目录可写、工具链只读，未经 Git mutation 授权的现有 `.git` 保持只读。Windows 沙箱任务始终离线，包括 `development` profile；不授予网络能力、不暴露宿主 Cargo 配置和凭据。Cargo 只使用已缓存并经过锁文件校验的公开依赖，缺失的当前目标依赖需先在可信环境准备。服务自身的 MCP、LAN Transfer 和宿主 ngrok 连接不受 Job 离线策略影响。

**原生隔离测试** 位于现有 Rust 单元测试及 `tests/integration.rs` 中，覆盖文件边界、Git 只读、继承句柄、子孙进程终止、权限回收及实际离线 Cargo 编译。启动或回收失败会明确报告，不自动回退到 host。工具链权限、链接、恢复和离线限制见 [执行模型](docs/EXECUTION.md#windows-原生-appcontainer) 与 [跨平台支持](docs/PORTABILITY.md)。

**固定 Worker 自检** 仍可在停服后运行 `.\target\windows-stage\debug\endlessvibe.exe --probe-windows-sandbox`。它验证固定 `hash_input` 协议、低权限令牌、Job Object、受限日志和恢复记录；受信 worker 还进行 EXE 摘要、PE 导入、System32 映像映射及离线嵌入式签名诊断。这些检查的具体保障与限制见跨平台文档，不代替一般命令的 Jobs 验收。

- 本机 MCP（OAuth）：`http://127.0.0.1:20000/mcp`，**只监听 loopback**，不是供局域网设备直接调用的 HTTP 地址。
- 本机 Dashboard：`http://127.0.0.1:20001/nodes`，**只允许本机访问**。
- 局域网通信：TCP **20002 / TLS 1.3**；自动发现：UDP **20003 / multicast**。仅在可信 LAN 内放行所需端口。
- 父节点在 Nodes 页面主动发起配对请求，双方核对同一个六位验证码后，**仅子节点点击“同意”或“拒绝”**，父节点自动完成配对，不必二次批准。配对默认**不授予任何 Project 权限**；自动发现不等于授权。
- 后续在 Windows 的 **Projects** 页面添加 Project（支持配置热重载），然后在 **Nodes → Trusted Nodes → 管理 Project 授权** 明确勾选权限并保存。**新增项目不会自动开放给 Linux**；授权修改立即生效，无需重新配对，且可以随时撤销。

`--lan-only` 不允许同时设置 `--public-url` 或 `--bind`。如需其他设备直接通过 MCP HTTP 接入，仍须使用 HTTPS 保护的地址；**不要在局域网通过纯 HTTP 发送 OAuth Bearer Token**。

详见 [LAN Transfer 节点配对](docs/TRANSFER.md)。

## Usage

### Read and edit files

```text
list_workspaces
  → list_projects(workspace)
  → read_file(workspace, project, path)
  → apply_patch(workspace, project, path, expected_sha256, edits)
```

已有文件必须提供读取时返回的 `expected_sha256`；创建文件使用 `MISSING`。Patch 使用精确文本替换，防止覆盖并发修改。

### Run a job

```json
{
  "workspace": "projects",
  "project": "website",
  "program": "cargo",
  "args": ["test", "--locked"],
  "request_id": "website-test-001",
  "timeout_seconds": 120
}
```

命令会返回 Job ID。之后通过 `get_job` / `get_job_output` 查询结果；网络中断时先查询原请求状态，**不要换一个请求 ID 盲目重放**。

### Review and commit changes

```text
git_status
  → git_diff(paths)
  → Review changes
  → git_commit(paths, expected_head, expected_diff_sha256)
```

`git_commit` 只提交通过审查的路径，不会默认执行 Push；远端推送需要额外启用 Project 权限。

## Architecture

```mermaid
flowchart TB
    Client["ChatGPT / MCP Clients"] -->|"OAuth 2.0 + PKCE"| MCP["Rust MCP Server"]
    Browser["Local Browser"] --> Dashboard["Dashboard · 127.0.0.1:20001"]
    MCP --> Runtime["Workspace / Project Runtime"]
    Dashboard --> Runtime
    Runtime --> Files["Files & Search"]
    Runtime --> Jobs["Command Jobs & Checkpoints"]
    Runtime --> Git["Git Review & Commit"]
    Runtime --> Store["SQLite State & Audit"]
    Runtime -.->|"Optional TLS pairing"| Nodes["Remote EndlessVibe Nodes"]
```

- **Rust**：Axum、Tokio、rmcp、Rusqlite、Rustls。
- **Web UI**：随 Rust 服务一起分发的静态资源；无需额外部署前端服务。
- **State**：SQLite 存储认证、任务、操作和节点状态；敏感内容按不同接口进行脱敏或限制保留。
- **Isolation**：项目锁、文件系统边界以及可配置的命令执行后端。

## Configuration

默认配置与状态路径随平台变化：

| 平台 | 配置文件 | 私有状态目录 |
| --- | --- | --- |
| Linux | `~/.config/endlessvibe/config.toml` | `~/.local/state/endlessvibe` |
| Windows | `%APPDATA%\EndlessVibe\config.toml` | `%LOCALAPPDATA%\EndlessVibe` |
| macOS | `~/Library/Application Support/EndlessVibe/config.toml` | `~/Library/Application Support/EndlessVibe/state` |

Linux/macOS 遵循 `XDG_CONFIG_HOME`、`XDG_STATE_HOME`；Windows 使用 `APPDATA`、`LOCALAPPDATA`。可用 `--config` 指定配置路径。运行环境中的 `ENDLESSVIBE_BIND`、`ENDLESSVIBE_PUBLIC_URL` 会覆盖文件中的地址，命令行地址参数优先于这些环境变量。

配置中的 Workspace / Project 示例：

```toml
[[workspaces]]
id = "projects"
path = "/home/user/projects"

[[workspaces.projects]]
id = "website"
path = "website"
allow_write = true
allow_exec = true
allow_git_commit = true
allow_git_mutation = false
allow_git_push = false
execution_profile = "isolated"
```

可用的 Project 执行模式：

| Profile | 用途 |
| --- | --- |
| `isolated` | 受限开发环境，使用当前已配置的 Bubblewrap 或 Windows AppContainer |
| `development` | 可信开发项目，仍受执行后端约束；Windows AppContainer 始终离线 |
| `trusted_host` | 显式高风险选项：配合已确认的 Host 后端，允许运行 PATH 中已安装的本机工具 |

`trusted_host` **不是默认功能**。它要求全局启用无沙箱的 Host 执行，会影响其他可执行项目的隔离安全性；仅应在受控环境中使用。

完整配置示例见 [config/config.example.toml](config/config.example.toml)，执行模式与工具链说明见 [docs/EXECUTION.md](docs/EXECUTION.md)。

## Security

EndlessVibe 是**单用户、自托管**开发工具，不应作为允许不可信用户共享宿主权限的多租户服务。

- **授权优先**：私有 MCP 工具需要 OAuth 认证及对应 Scope；Project 权限显式配置。
- **路径保护**：文件 API 不允许逃逸 Project 根目录或通过符号链接绕过边界。
- **默认执行隔离**：Bubblewrap 不默认暴露真实 HOME、SSH 凭据、Docker Socket 或服务状态目录。
- **危险操作需显式开启**：Shell、Docker 修改、Git Push 和 Host 执行均有独立条件或配置约束。
- **可追溯**：Job、操作日志和恢复状态持久化；未知执行结果不能推断为“未执行”。
- **Dashboard 仅监听本机**：不要通过公网反向代理暴露管理界面。

请阅读 [安全模型](docs/SECURITY.md) 和 [执行模型](docs/EXECUTION.md)，再开放外部 MCP 客户端或切换执行后端。

## Development

```bash
cargo fmt --check
cargo test --locked
cargo build --release
```

提交代码时建议按功能拆分 Commit，在每次修改后检查 `git diff` 和相应测试。项目遵循 MIT License，欢迎通过 Issue / Pull Request 讨论问题与改进。

## Documentation

| 文档 | 内容 |
| --- | --- |
| [Tools](docs/TOOLS.md) | MCP 工具及参数 |
| [Execution](docs/EXECUTION.md) | Bubblewrap、Host、Toolchain、Job 与预检 |
| [Platform Support](docs/PORTABILITY.md) | Linux、Windows、macOS 的移植状态与平台限制 |
| [Security](docs/SECURITY.md) | 权限、安全边界与部署注意事项 |
| [OAuth](docs/OAUTH.md) | OAuth 2.0、PKCE 与客户端接入 |
| [Git Recovery](docs/GIT_RECOVERY.md) | Git 审查、提交与恢复 |
| [Node Transfer](docs/TRANSFER.md) | 节点发现、授权、请求去重与诊断 |
| [Cloudflare Tunnel](docs/DOCKER_TUNNEL.md) | HTTPS / Tunnel 部署 |

## License

EndlessVibe 基于 [MIT License](LICENSE) 开源。
