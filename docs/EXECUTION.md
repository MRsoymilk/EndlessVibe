# 命令执行后端

## 默认 bubblewrap

`backend="bubblewrap"` 配合 `allow_exec=true` 才能启动任务。首次检查 `command -v bwrap`，由管理员安装发行版维护的 bubblewrap，并确认非特权 user namespace 可用。服务不提权、不执行 sudo，不存在自动不安全回退。

沙箱中 `/workspace` 是选中项目，`/cache` 是该项目独立的私有构建缓存目录。`HOME=/tmp/home`，`CARGO_HOME=/cache/cargo`，默认 PATH `/usr/local/bin:/usr/bin:/bin`。系统可执行/库目录只读，项目 `.git` 再覆盖为只读。源文件本身可写，所以构建脚本能够改变选中项目；这是执行权限的一部分。

默认不暴露真实 HOME、整个 `/etc`、SSH、DBus、Wayland、云凭据或服务状态。需要 UI、GPU、网络、跨项目依赖或其他宿主机资源的命令可能失败；默认不会开放这些资源。文件读写 MCP API 的敏感名称过滤不是 Shell 的文件访问过滤：有执行权限便可以访问沙箱内该项目的全部文件。

网络默认关闭。依赖还未下载时 Cargo/npm 等失败是预期行为：在本机管理独立缓存，或经过风险确认后将 `execution.allow_network=true`。该选项会允许访问网络（包括可能可达的内网），不是单纯允许 crates.io。系统代理环境变量不会自动传给子进程。

## Rustup / 自定义 SDK

当 cargo/rustc 在 `/usr/bin` 时可以直接调用。使用 Rustup 时，默认不会挂载整个 `~/.cargo` 或 `~/.rustup`。在本机获取真实工具链目录，然后精确配置只读挂载，例如：

```toml
[execution]
backend = "bubblewrap"
path = "/opt/rust/bin:/usr/local/bin:/usr/bin:/bin"
readonly_mounts = [{ source = "/home/vv/.rustup/toolchains/你的真实工具链目录", target = "/opt/rust" }]
```

将这些字段合并到已有 `[execution]`，不要重复声明整个表；示例路径必须替换为真实目录。允许的目标路径限制在 `/opt/...` 或 `/cache-readonly/...`，不得通过 mount 暴露服务 config/state。额外 SDK 只读挂载扩大可见范围，需要自行检查其中是否有密钥。程序查找与共享库依赖仍需在目标环境验证。

## Shell 与白名单

`run_command` 接受程序名+参数，不默认使用 `sh -c`。程序名必须属于 `allowed_programs`。`run_shell` 需要明确 `allow_shell=true`，不是通过往 program 白名单里加 bash 来隐式开启。

白名单并非安全边界：Python、Cargo build script、Makefile、编译器插件等都能执行其他程序。安全限制主要来自隔离环境与管理员授予的工作区权限。工具描述/确认提示不是 OS 权限控制。

## 显式 host 模式

```toml
[execution]
backend = "host"
acknowledge_unsafe_host_execution = true
allow_shell = false
```

仅在你明确需要并接受宿主机用户权限时使用。虽然环境变量会清理、程序 argv 分离、进程组受管理，但它仍能访问该 UID 可访问的文件、其他项目、Token 状态和网络，也能自行运行 Git push/删除等命令；**文件工具权限与“没有 push 工具”无法约束已授权的任意宿主机代码执行**。`allow_network` 只配置 bubblewrap 的网络 namespace，不会给 host 模式加网络隔离。

初次初始化的 `--unsafe-host-exec` 是等价的显式危险选项，默认不启用。服务禁止 root；不要为排错自行删除这个检查。需要面对不可信代码时使用独立用户/VM/经审计的容器执行器，而非本模式。

## 生命周期、持久化与资源

提交动作不等待编译结束，而是保存 Job 后返回 job_id。运行期间同一项目的文件/Git操作返回 WORKSPACE_BUSY，避免在构建运行时同时改写。任务查询本身不会争抢该项目锁。

超时/取消对进程组发送 TERM，再 KILL；bubblewrap 带父进程退出终止策略。host 模式中自行 daemonize、setsid 或逃离进程组的恶意程序不在强保证范围，不能宣称等价于 cgroup/VM 管控。服务重启不自动 replay 任何命令。

默认最大两个并发任务、120 秒、8 GiB RLIMIT_AS、256 RLIMIT_NPROC。RLIMIT_AS 是地址空间而非 RSS，NPROC 可能受同 UID 已有进程影响；这些不是精确的每 Job cgroup 配额，也没有磁盘配额。大型 C++/Bevy 编译可能需要在本机调整。输出只保留配置大小的最近内容；缓存和项目生成文件不会自动删除。
