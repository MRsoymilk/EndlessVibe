# 安全边界与运行前检查

本版是**单管理员、本机项目开发后端**，不是多租户安全平台。内置 OAuth、文件/Git隔离逻辑没有经过独立安全审计；不能因为检查脚本通过就声称安全或 ChatGPT 注册成功。

## 访问控制

所有实际工具执行必须带有效 Token。`initialize`、`notifications/initialized`、`ping` 和 `tools/list` 可匿名访问，只返回静态协议信息与工具描述，让客户端在首次授权前发现 OAuth 策略。未授权的 `tools/call` 返回 `isError=true` 和 `_meta["mcp/www_authenticate"]`，HTTP `200` 仅表示成功传递认证错误，不表示工具执行成功；其他未授权请求仍返回 `401`。Token 在每次工具调用重新检查，按工具 scope 拒绝权限不足的调用；未知工具的策略默认最严格。Workspace 根必须显式配置，Project 必须挂载在对应根目录内；写入、执行和提交权限属于 Project。所有 OAuth 客户端属于同一个 owner，不实现每客户端独立项目列表。

公开页面仅返回状态、版本、监听端口、工具名和数量；不返回真实项目路径、文件/命令内容、access token、owner key 或审计日志。日志记录 HTTP method/path/status/耗时、受限 MCP method 分类、认证状态和 OAuth 错误码，不记录 query、Authorization、OAuth表单或Shell参数。HTTP/Cloudflare 基础设施自身的日志策略需要管理员另行检查。

Scope、MCP readOnlyHint 等标注辅助客户端做确认，不是 OS 沙箱。本机获得管理员 key 的人可以授权访问已配置的 Workspace/Project；所有凭据都应保持私有。已经返回给 ChatGPT 的代码/日志无法通过后续本地 Token 撤销收回。

## 文件与并发

文件 API 用持有的目录 FD 和 openat2/RESOLVE_BENEATH/NO_SYMLINKS/NO_MAGICLINKS 访问，禁止绝对路径、`..`、符号链接、硬链接和常见敏感名称。新文件原子创建不得覆盖旧文件；已有文件先比对摘要并备份。常见名称过滤不意味着能识别所有秘密内容，不能把源码中的密钥当成自动脱敏。

每个 Project 有独立服务级互斥锁，串行化该 Project 的文件、命令和 Git 操作；同一 Workspace 下互不重叠的 Project 可以并行。外部编辑器不遵守该锁；两次 hash 检查与 rename 之间仍有外部修改的极短竞态。它是乐观冲突检测，并非通用文件 CAS，更不是同 UID 恶意进程隔离。需要更严格语义时，应在独立工作副本/VM 中执行并人工合并。

## 命令与 Git

默认 bubblewrap 不挂载服务私有状态、整个 HOME 或 Docker socket，网络关闭，`.git` 只读；缺少支持时失败，不降级。Project 可显式设置 `allow_git_mutation=true`，此时仅该 Project 自身 `.git` 在 sandbox 中可写，用于本地 branch/switch/merge/commit 等操作；该权限要求 write+exec。`run_command git` 仍拒绝网络 Git 子命令，且真实 HOME/SSH 凭据不会因此暴露。构建脚本、解释器和Shell都是任意代码执行，需要显式授权。能执行代码的客户端可读写所选项目的全部内容；应把项目里的 `.env` 等真实凭据移出执行环境。

host 模式显式确认后拥有服务用户权限，可能读到其他 Project、Workspace 外文件和本机凭据，不受 Workspace/Project 路径语义的实际 OS 限制。不允许 root 服务；不要通过 Docker `--privileged` 等措施来掩盖隔离故障。bwrap共享宿主机内核，不是 VM，也未配置完整 seccomp/cgroup 磁盘配额；需持续维护 OS、SDK、Git、bwrap 与依赖。

专门 Git 工具禁用或拒绝外部 helpers/includes/filter 等配置，不推送、不重置、不丢弃不相关修改；日志和 index.lock 恢复必须人工确认。Git 元数据访问仍属于可信仓库假设，详情见 `GIT_RECOVERY.md`。

## 本机状态

服务账户自己持有的配置/状态目录 0700，凭据和数据库 0600，SQLite sidecar 由 umask077保护。Token/授权码使用随机值及哈希存储；任务输出、文件备份则包含原始内容，必须作为敏感文件管理。数据库的 WAL 不是加密。

最近100个已完成 Job输出、7天请求去重、10000条左右审计构成默认逻辑保留规则。备份和编译缓存没有自动删除/磁盘配额；不会为了节省空间自动丢失用户恢复资料。长期使用需要管理员检查磁盘并安排私有备份/保留策略；SQLite 文件空间也不保证删除记录后立即缩小。

## 部署前自查

确认目标平台完成真实 Cargo 构建和测试、Workspace/Project 列表只含授权目录、配置和状态位于项目外、以非root运行、私有数据不匿名可见、Cloudflare 转发OAuth路径、HTTPS认证有效、Scope正确、沙箱能访问必要工具链但不能读owner.key、取消/超时符合预期、Git拒绝预暂存冲突且保持用户差异、崩溃恢复流程已理解。

真实 ChatGPT OAuth 集成和目标 Gentoo 沙箱没有在此打包环境验证。外部审计、多用户 RBAC、成熟 IdP、跨主机执行器等不在本次源代码交付范围。
