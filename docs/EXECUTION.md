# 命令执行后端

## 默认 bubblewrap

`backend="bubblewrap"` 配合 `allow_exec=true` 才能启动任务。首次检查 `command -v bwrap`，由管理员安装发行版维护的 bubblewrap，并确认非特权 user namespace 可用。服务不提权、不执行 sudo，不存在自动不安全回退。

构建新版本后可以在不执行项目代码的情况下单独验证 namespace 与基础 mount：

```bash
./target/release/endlessvibe --check-sandbox
```

该检查使用与任务相同的基础 bubblewrap 参数和只读系统挂载，但不挂载任何 Project、不运行项目代码。默认 `execution.auto_discover_toolchains=true`：EndlessVibe 会解析宿主程序及符号链接的真实安装位置，对安全的 `/opt/<toolchain>` 前缀自动建立同路径只读 mount，并自动把 `/usr/lib*/postgresql*/bin`、安全 `/opt/.../bin` 等已发现目录加入本次 sandbox PATH；不再要求为 Gentoo `/usr/bin/rustc -> /opt/rust-bin-*` 这类布局手写 mount。它还会在 sandbox 内逐项解析 `execution.allowed_programs ∪ execution.required_programs`。`allowed_programs` 控制全局可直接启动的顶层程序；项目 manifest 自动检测到的受支持工具也可在该项目内使用。`required_programs` 表示服务级环境必须存在，适合声明全局门禁。成功时会打印每个程序的 sandbox 路径，例如：

```text
sandbox program: cargo -> /opt/rust/bin/cargo
sandbox program: rustc -> /opt/rust/bin/rustc
sandbox program: git -> /usr/bin/git
bubblewrap sandbox probe: ok (9 configured programs visible)
```

只要配置在 `allowed_programs` 或 `required_programs` 中的任意程序不可见，`--check-sandbox` 就失败并明确列出缺失项，并额外标记 `required_missing`。这样可在服务承担真实 Job 前发现“宿主机能运行 cargo，但 bubblewrap PATH 看不到 cargo”这类配置错误。

沙箱中 `/workspace` 是选中项目，`/cache` 是该项目独立的私有构建缓存目录。`HOME=/tmp/home`，`CARGO_HOME=/cache/cargo`，默认 PATH `/usr/local/bin:/usr/bin:/bin`。系统可执行/库目录只读。项目源文件可写；`.git` 默认再覆盖为只读。只有 Project 显式设置 `allow_git_mutation=true` 时，项目自身 `.git` 才保持可写，从而允许 `git switch`、`git merge`、`git branch`、`git add`、`git commit` 等本地仓库变更。

`allow_git_mutation` 需要同时启用 `allow_write=true` 与 `allow_exec=true`，默认关闭。即使开启，`run_command` 仍拒绝 `git push/fetch/pull/clone/ls-remote/remote/submodule` 等网络 Git 子命令；真实 HOME、SSH 凭据和服务状态仍不挂载，sandbox 网络仍由 `execution.allow_network` 独立控制。远端 push 使用单独的 `allow_git_push=true` 和宿主机侧专用 `git_push`，与 bubblewrap 网络权限无关；HTTPS 通过服务账户 Git credential helper，SSH 通过服务账户 HOME/SSH agent。`run_shell` 是显式的广泛执行权限，启用后不能依赖这层 argv 子命令过滤作为安全边界。

默认不暴露真实 HOME、整个 `/etc`、SSH、DBus、Wayland、云凭据或服务状态。需要 UI、GPU、网络、跨项目依赖或其他宿主机资源的命令可能失败；默认不会开放这些资源。文件读写 MCP API 的敏感名称过滤不是 Shell 的文件访问过滤：有执行权限便可以访问沙箱内该项目的全部文件。

网络默认关闭。依赖还未下载时 Cargo/npm 等失败是预期行为：在本机管理独立缓存，或经过风险确认后将 `execution.allow_network=true`。该选项会允许访问网络（包括可能可达的内网），不是单纯允许 crates.io。系统代理环境变量不会自动传给子进程。

## 工具链自动发现与手工覆盖

新 Project 会在最多 3 层目录内检查常见 manifest：`Cargo.toml`、`CMakeLists.txt`、`pyproject.toml` / `requirements.txt`、`package.json`、`project.godot`、`go.mod` 与 compose 文件。Rust、CMake、Python、Node、Godot、Go 会自动形成项目工具链，用于构造该项目 Job 的安全 PATH / readonly mount；它们不会因此成为每个 Job 的强制门禁。对 `python3 relative/script.py` 和显式 Bash `-c`，EndlessVibe 会有界扫描当前脚本中直接引用的受支持子进程工具，并仅把这些工具加入本次 preflight。例如交易 verifier 同时引用 Cargo、Godot 与本地 PostgreSQL 工具时会在 Job 创建前一次检查，而普通 `python3 -c` 不会因为项目同时存在 PostgreSQL 代码而被误拦。manifest 和脚本检测均限制为小文件，并跳过 `.git`、`target`、`node_modules`、虚拟环境和构建目录。

自动发现只会额外暴露已识别程序所需的系统目录与 `/opt/<toolchain>` 安装前缀，不会自动挂载 HOME、凭据目录、数据库 data directory、socket 或 Docker socket。缺失工具会区分为 `host_missing`（宿主未安装）与 `sandbox_missing`（宿主存在但 sandbox 不可见）；Dashboard 和结构化 Job preflight 错误会给出宿主路径或只读的发行版包查询提示。EndlessVibe 不执行 sudo/emerge/apt 等宿主包安装。可设置 `auto_discover_toolchains=false` 恢复完全手工模式；显式 `readonly_mounts` 与 `execution.path` 仍可作为特殊 SDK 的覆盖配置。

使用 Rustup 时，仍不会挂载整个 `~/.cargo` 或 `~/.rustup`，因为 `~/.cargo` 可能包含 registry 凭据。若自动发现无法安全定位 Rustup 工具链，应只挂载实际 toolchain 目录。

先在宿主机查询实际 Rust 工具链：

```bash
rustup which rustc
```

例如返回 `/home/user/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/rustc`，则将该 toolchain 根目录只读映射到 sandbox：

```toml
[execution]
backend = "bubblewrap"
path = "/opt/rust/bin:/usr/local/bin:/usr/bin:/bin"
readonly_mounts = [{ source = "/home/user/.rustup/toolchains/stable-x86_64-unknown-linux-gnu", target = "/opt/rust" }]
```

这样 sandbox 使用 `/opt/rust/bin/cargo` 和 `/opt/rust/bin/rustc`，无需暴露 `~/.cargo` 或整个 HOME。修改配置后重启 EndlessVibe，先执行 `--check-sandbox`；它现在会直接验证 cargo/rustc 是否可见。通过后再运行项目级 `cargo fmt --check`、`cargo check`、`cargo test --all-targets` 和 `cargo clippy --all-targets` 建立完整验证闭环。

将这些字段合并到已有 `[execution]`，不要重复声明整个表；示例路径必须替换为真实目录。允许的目标路径限制在 `/opt/...` 或 `/cache-readonly/...`，不得通过 mount 暴露服务 config/state。额外 SDK 只读挂载扩大可见范围，需要自行检查其中是否有密钥。程序查找与共享库依赖仍需在目标环境验证。

## Sandbox 内临时 PostgreSQL

`allow_network=false` 不妨碍**同一个 Job** 内的父进程和子进程使用该 Job 自己的 loopback namespace。因此集成测试不必挂宿主 Docker socket，也不必连接开发数据库：测试脚本可以在 sandbox 内自行启动一次性 PostgreSQL，并只监听 `127.0.0.1`。

若 PostgreSQL 已安装在 `/usr/bin` 和系统库目录，通常无需额外 mount；若使用自定义 PostgreSQL 安装，应只读挂载包含 `bin/`、所需 `lib/`/`share/` 的最小安装前缀，例如：

```toml
[execution]
path = "/opt/rust/bin:/opt/postgres/bin:/usr/local/bin:/usr/bin:/bin"
required_programs = ["cargo", "rustc", "initdb", "postgres", "pg_isready", "createdb"]
readonly_mounts = [
  { source = "/home/user/.rustup/toolchains/stable-x86_64-unknown-linux-gnu", target = "/opt/rust" },
  { source = "/opt/postgresql-16", target = "/opt/postgres" },
]
```

挂载后，sandbox 内至少应能解析 `initdb`、`postgres`、`pg_isready`、`createdb`。这些程序不必为了测试脚本的子进程调用而额外加入顶层 `allowed_programs`；将它们列入 `required_programs` 即可让 `--check-sandbox` fail-closed 验证，同时不扩大 `run_command` 顶层程序白名单。它们仍必须能从 `execution.path` 找到。修改 service-level execution 配置后需重启 EndlessVibe。

不要只读/读写挂载宿主 PostgreSQL 的数据目录、认证文件、Unix socket 或开发数据库，也不要为了测试挂 `/var/run/docker.sock`。测试应在 `/cache` 或其他 Job 私有可写目录初始化新数据目录，使用随机 loopback 端口，并在 Job 结束时终止数据库进程、删除临时数据。这样即使 `execution.allow_network=false`，Server、Godot/Native 与临时 PostgreSQL 仍可在同一个隔离 Job 内完成真实数据库联测，同时不暴露宿主内网或开发库。

## Shell 与白名单

`run_command` 接受程序名+参数，不默认使用 `sh -c`。程序名必须属于 `allowed_programs`。`allowed_programs` 中每一项必须是唯一的简单可执行名（如 `cargo`、`git`），不能写绝对路径；实际位置由 `execution.path` 与只读 mount 决定。`run_shell` 需要明确 `allow_shell=true`，不是通过往 program 白名单里加 bash 来隐式开启。

每个 `run_command` / `run_shell` 还可以提供 `preflight_programs`。这些程序会在 **Job ID 创建、command slot 占用、Project lock 获取、Job 持久化之前**在实际执行后端中做可见性检查；缺失时以 `JOB_PREFLIGHT_FAILED` fail-fast，不启动目标命令。直接运行 `cargo` 会自动把 `rustc` 加入门禁。Rust + PostgreSQL 联合验收应显式传 `cargo,rustc,initdb,postgres,pg_isready,createdb`；成功的 preflight 路径和 toolchain 状态会持久化到 Job 的 `preflight` 字段。该字段只做环境门禁，不绕过 `allowed_programs` 对顶层 `run_command` 的权限控制。

Job 启动后还会得到 `ENDLESSVIBE_JOB_SUMMARY`。bubblewrap 下该路径位于当前 Project 的私有 `/cache` 挂载；脚本可以写入 ≤1 MiB JSON object 汇报 database、cleanup、source fingerprint 等验收结果。服务在 Job 终止后读取并脱敏到持久 Job `summary`，记录 `summary_capture` 后删除临时文件；不会把宿主 state 路径返回给客户端，也不会跟随 symlink。

白名单并非安全边界：Python、Cargo build script、Makefile、编译器插件等都能执行其他程序。安全限制主要来自隔离环境与管理员授予的 Project 权限。工具描述/确认提示不是 OS 权限控制。

## 显式 host 模式

```toml
[execution]
backend = "host"
acknowledge_unsafe_host_execution = true
allow_shell = false
```

仅在你明确需要并接受宿主机用户权限时使用。虽然环境变量会清理、程序 argv 分离、进程组受管理，但它仍能访问该 UID 可访问的文件、其他项目、Token 状态和网络，也能自行运行 Git push/删除等命令；**文件工具权限与专用 push 权限无法约束已授权的任意宿主机代码执行**。`allow_network` 只配置 bubblewrap 的网络 namespace，不会给 host 模式加网络隔离。

初次初始化的 `--unsafe-host-exec` 是等价的显式危险选项，默认不启用。服务禁止 root；不要为排错自行删除这个检查。需要面对不可信代码时使用独立用户/VM/经审计的容器执行器，而非本模式。

## 配置热重载

Dashboard 的 Project 新增与权限修改在写入并完整验证 `config.toml` 后，会立即重建 Workspace/Project 授权表并原子替换 Runtime 中的新操作视图，无需重启服务。手工编辑配置后也可以在 Config 页面执行 `Reload project config`，对应本地 `POST /api/config/reload`。

热重载只覆盖 Workspace/Project 授权模型。`server`、`security`、`limits`、`execution`、`git` 等服务级配置在进程启动时固定；如果这些字段与启动时持久配置不同，reload 返回 `RELOAD_RESTART_REQUIRED`，旧授权表继续生效，必须重启 EndlessVibe。

对于同一 Workspace/Project 且路径未变化的 Project，新授权对象复用旧 Project lock。因此 reload 前已经运行的文件、Git 或命令操作继续持有原来的 `Arc<Project>` 并完成，新操作使用新权限但仍与旧操作共享同一把锁，不会在同一目录并发。reload 时已被删除或改路径、但仍有操作在运行的旧 Project 会暂时保留在 retired 集合中供 shutdown 等待，不重新接受新操作。

如果 Project 配置已成功写入但由于同时存在服务级配置变化而无法热重载，Dashboard 返回成功写入结果并设置 `requires_restart=true` 与 `reload_warning`；客户端不应重复提交同一配置写操作。

## 生命周期、持久化与资源

提交动作不等待编译结束，而是保存 Job 后返回 job_id。同一 Project 运行任务期间，文件/Git 操作返回 `PROJECT_BUSY`；同一 Workspace 下其他 Project 使用独立锁，可以并行。任务查询本身不争抢 Project 锁；命令总并发仍受 `limits.max_jobs` 限制。

超时/取消对进程组发送 TERM，再 KILL；bubblewrap 带父进程退出终止策略。host 模式中自行 daemonize、setsid 或逃离进程组的恶意程序不在强保证范围，不能宣称等价于 cgroup/VM 管控。服务重启不自动 replay 任何命令。

默认最大两个并发任务、120 秒和 8 GiB RLIMIT_AS。`max_processes=256` 目前只在显式 host 后端作为 RLIMIT_NPROC 使用；bubblewrap 启动器不再设置 RLIMIT_NPROC，因为 Linux 按真实 UID 的全部线程/进程计数该限制，在桌面用户已有任务数较多时会让 bwrap 自己的 namespace `clone()` 以 EAGAIN 失败。bubblewrap 下若需要严格的每 Job 进程数上限，应使用外部受委托的 cgroup v2 `pids.max` 等机制，而不是对启动器施加 RLIMIT_NPROC。

RLIMIT_AS 是地址空间而非 RSS；这些限制也不是完整的每 Job cgroup 配额，并且没有磁盘配额。大型 C++/Bevy 编译可能需要在本机调整。输出只保留配置大小的最近内容；缓存和项目生成文件不会自动删除。
