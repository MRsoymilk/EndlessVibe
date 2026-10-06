# MCP 工具参数与工作流

接口由 `src/tools/types.rs` 的 Rust 类型生成 JSON Schema。所有私有工具都需要有效 OAuth Bearer；Project 的写入、执行和 Git 权限还需由本地配置允许。服务维护显式 `tool_schema_revision`；任何工具名、参数 schema、安全 metadata 或语义变化都必须递增该 revision。`hello` 与 `get_service_status` 都会返回 revision 和工具数量，用于识别客户端缓存旧 schema。

## 定位模型

EndlessVibe 使用两层定位：

```text
Workspace = 管理根目录
Project   = Workspace 下的实际操作单元
```

先列出根节点：

```json
{}
```

调用 `list_workspaces`。

再列出指定根下的 Project：

```json
{"workspace":"projects"}
```

调用 `list_projects`。后续 Project 操作都需要同时传入：

```json
{"workspace":"projects","project":"BAfter"}
```

Project 路径来自本机配置，所有 `path` / `cwd` 都相对于 Project 根目录。

## 结构化错误

MCP 工具失败时仍保留人类可读的文本错误，同时 `structuredContent` 返回稳定结构：

```json
{"code":"FILE_CONFLICT","message":"expected_sha256 does not match current content; read again, do not overwrite","retryable":true,"details":{"expected_sha256":"...","actual_sha256":"..."}}
```

Dashboard 写操作使用相同的 `code / message / retryable / details` JSON。客户端应根据 `code` 做流程判断，不要解析 `message` 文案。已迁移的稳定 code 包括配置/文件/patch/Git 冲突、Project busy/授权/权限、Job idempotency/slots 以及受控 Git push 的失败类型；未分类错误统一为 `OPERATION_FAILED`。

## 项目与文件

`inspect_project`：

```json
{"workspace":"projects","project":"BAfter"}
```

`read_file`：

```json
{"workspace":"projects","project":"BAfter","path":"CMakeLists.txt","start_line":1,"max_lines":200}
```

返回完整文件 SHA-256、分页内容、行数和 `next_line`。分页内容的 SHA-256 仍对应完整文件。

`list_directory`：

```json
{"workspace":"projects","project":"BAfter","path":"src","offset":0,"limit":200}
```

`search_code`：

```json
{"workspace":"projects","project":"BAfter","path":".","query":"FormInfoPower","regex":false,"case_sensitive":true,"max_results":100}
```

`write_file`：

```json
{"workspace":"projects","project":"BAfter","path":"docs/new.md","content":"# New document\n","expected_sha256":"MISSING","create_parents":true}
```

新文件使用 `MISSING`；已有文件必须使用最近一次 `read_file.sha256`。

`apply_patch`：

```json
{"workspace":"projects","project":"BAfter","path":"docs/new.md","expected_sha256":"真实摘要","edits":[{"old_text":"New document","new_text":"Updated document","expected_occurrences":1}]}
```

`apply_patch` 只做精确文本替换，不接收 unified diff。`create_directory` 同样需要 `workspace + project + path`。

## 命令与 Shell

`run_command`：

```json
{"workspace":"projects","project":"mountain_and_sea","program":"cargo","args":["check"],"cwd":".","request_id":"mas-check-001","timeout_seconds":120,"preflight_programs":["cargo","rustc"],"task_id":"combat-refactor","stage":"validate-core"}
```

`preflight_programs` 是可选的 Job 前置工具链门禁。EndlessVibe 会在创建 Job ID、占用 command slot、获取 Project lock 和写入 Job 记录之前，在实际执行后端验证这些程序可见；缺失时直接返回结构化 `JOB_PREFLIGHT_FAILED`，不会启动长任务。直接执行 `cargo` 时会自动同时要求 `rustc`。如果 Job 自己启动本地 PostgreSQL，联合 Rust + PostgreSQL 验收可使用：

```json
{"preflight_programs":["cargo","rustc","initdb","postgres","pg_isready","createdb"]}
```

常规开发不需要每次传 `network` 或 `environment`。在 `config.toml` / Dashboard 中把受信 Project 一次性设为：

```toml
[[workspaces.projects]]
id = "mountain_and_sea"
path = "mountain_and_sea"
execution_profile = "development"
# 只有项目长期需要固定变量时才配置；Docker 驱动的测试通常不需要。
# environment = ["TEST_DATABASE_URL=postgres://user:password@127.0.0.1:19031/test"]
```

之后原有 Docker/Compose 验证脚本可按普通命令运行；EndlessVibe 会自动继承宿主网络，并在 Docker socket 可用时设置 `DOCKER_HOST`。例如无需额外数据库参数：

```json
{"workspace":"projects","project":"mountain_and_sea","program":"python3","args":["game_mas/tools/verify_v26_trade_godot.py"],"cwd":".","request_id":"trade-001","timeout_seconds":120}
```

Job 级 `network` / `environment` 仍保留为覆盖能力：`development` Job 可用 `network=false` 临时隔离；Project environment 会先继承，再由 Job 同名变量覆盖。`HOME`、`PATH`、`CARGO_HOME`、`ENDLESSVIBE_JOB_SUMMARY` 等运行时变量不可覆盖。摘要和日志只记录环境变量名，不记录值。

每个已接受 Job 还会收到环境变量 `ENDLESSVIBE_JOB_SUMMARY`。bubblewrap 后端中的值是 Job 专属 `/cache/job-summary-<job_id>.json`；测试/构建脚本可以在退出前写入一个不超过 1 MiB 的 JSON object。EndlessVibe 在进程结束后通过 `O_NOFOLLOW` + owner/link/type/size 检查读取它，对常见 credential/password/token 字段脱敏，然后保存到 Job 的 `summary`；读取状态和临时文件删除结果保存在 `summary_capture`。未写该文件的普通 Job 记录为 `not_reported`，不会因此失败。不要把 secret 放入 summary，即使服务端会执行防御性脱敏。

通过 preflight 后任务立即返回 `job_id`。请求去重键是 `workspace + project + request_id`，因此不同 Project 可以使用相同 request_id 而不会互相复用。`preflight_programs` 属于请求指纹的一部分，同一 request_id 不能用不同门禁条件重试。`task_id` 与 `stage` 必须同时提供或同时省略；提供后 Job 会自动挂到对应阶段 checkpoint。

`run_shell`：

```json
{"workspace":"projects","project":"BAfter","script":"printf '%s\n' 'hello'; pwd","cwd":".","request_id":"shell-001","timeout_seconds":10,"preflight_programs":[]}
```

`run_shell` 只有在 `execution.allow_shell=true` 时可用。

一个 Project 在命令运行期间持有自己的操作锁；同一 Project 的文件/Git 操作会返回 `PROJECT_BUSY`。不同 Project 的锁独立，可以并行，命令总并行数仍受 `limits.max_jobs` 限制。

## Job

`get_job` / `cancel_job`：

```json
{"job_id":"真实 Job ID"}
```

`get_job_output`：

```json
{"job_id":"真实 Job ID","offset":0,"limit":8192}
```

默认每页 8 KiB，单次最多 32 KiB。成功的终态 `get_job` 会返回 `result_summary`，对 Cargo/Rust 风格的 `test result:` 自动汇总 suite 数、passed/failed/ignored/measured/filtered_out，并不再附带原始 tail；失败/超时/中断仍附带最多 4 KiB 的小 tail。只有诊断确实需要原始日志时才继续按 `next_offset` 分页读取。完整 ring buffer 仍保存在 EndlessVibe，MCP 分页只是限制单次返回体。

`list_jobs` 可以按 Workspace、Project、`task_id` 过滤：

```json
{"workspace":"projects","project":"BAfter","task_id":"release-031","limit":20}
```

终态包括 `succeeded`、`failed`、`timed_out`、`cancelled`、`interrupted`。

为降低长会话发生 stream/input 错误的概率，MCP 大结果采用“小响应、服务端保留完整数据”的模型：成功结果不再把完整 JSON 同时复制到 text content；`read_file` 单页最多 64 KiB / 1000 行，`search_code` 默认 50 条且单响应最多 200 条 / 64 KiB，`git_status` 最多返回 200 个变更项并给出 `total_entries/truncated`。需要更多内容时使用分页或更窄查询，而不是一次取完整日志、diff 或搜索结果。

## 长任务 Checkpoint

对可能跨多轮 ChatGPT stream 的工作，使用一个稳定 `task_id`，每个可独立审查的小阶段使用一个 `stage`：

```text
release-031
├── prepare
├── validate
├── merge
└── version-bump
```

每次 `run_command` / `run_shell` 带上：

```json
{"task_id":"release-031","stage":"validate"}
```

checkpoint 会保留最近 Job ID 及其状态。一个 stage 可以有多个 Job，默认最多保留该阶段最近 20 个 Job 引用。

阶段完成时，`git_commit` 继续传入相同的 `task_id + stage`。只有 commit 成功后，该阶段才进入 `committed`，并记录 `last_commit`。因此 `job_succeeded` 只表示校验命令完成，不表示整个阶段已经落盘。

stream 中断或新会话恢复时，优先调用 `continue_task`，一次获得当前阶段、最近 Job、最后 durable Git checkpoint 和下一步建议；它只读，不会自动执行或重试。知道 task_id 时可显式指定；只说“继续”时可省略 task_id，让 EndlessVibe 自动选择该 Project 最近更新的 Task：

```json
{"workspace":"projects","project":"BAfter"}
```

或：

```json
{"workspace":"projects","project":"BAfter","task_id":"release-031"}
```

典型 `recommended_action.kind`：`poll_job`、`checkpoint_stage`、`inspect_and_retry`、`start_next_stage`。需要完整阶段历史时再调用 `get_task_checkpoint`：

```json
{"workspace":"projects","project":"BAfter","task_id":"release-031"}
```

返回 `latest` 和最近阶段 `stages`，其中包含状态、Job ID/状态、最后 commit 和更新时间。

`list_task_checkpoints`：

```json
{"workspace":"projects","project":"BAfter","task_id":"release-031","limit":20}
```

也可以省略 `task_id` 查看最近任务恢复点。服务重启时，原本 `queued/running` 的 Job 会标记为 `interrupted`，关联 checkpoint 同步更新；Job 不会自动重放。

## Git

`git_status` / `git_log`：

```json
{"workspace":"projects","project":"BAfter"}
```

`git_diff`：

```json
{"workspace":"projects","project":"BAfter","paths":["src/example.cpp","README.md"],"offset":0,"limit":16384}
```

返回 `head`、明确的 `paths`、稳定 `diff_sha256`、`added_lines/removed_lines`、`total_bytes`、当前 `diff` 分页以及 `next_offset/has_more`。默认 16 KiB、单页最多 32 KiB；`has_more=true` 时应继续分页审查，所有页面共享同一个 review token，然后再 `git_commit`。

`git_commit`：

```json
{"workspace":"projects","project":"BAfter","paths":["README.md","src/example.cpp"],"message":"fix(example): handle missing value","expected_head":"git_diff 返回的 HEAD","expected_diff_sha256":"git_diff 返回的审查摘要","task_id":"release-031","stage":"version-bump"}
```

`git_commit` 只提交已审查文件；选中文件已有用户 staged 修改时拒绝，其他 staged 内容保留。不运行 hooks/filters/签名，不 push，不改写工作树。首次提交的 HEAD 使用 `UNBORN`。

`git_push`：

```json
{"workspace":"projects","project":"BAfter","remote":"origin","branch":"main","expected_head":"完整本地 branch commit SHA"}
```

必须为 Project 显式启用 `allow_git_push=true`。`expected_head` 必须是完整 40/64 位 Git OID；本地 branch 在调用前或 dry-run 后发生变化都会以 `GIT_CONFLICT` 拒绝，且不会执行真实 push。工具先查询远端同名 branch HEAD，再执行 `git push --dry-run`，通过后才做真实非 force push。成功结果返回 `remote_head_before`、`remote_head_after`、`commit` 和 `preflight=dry_run_passed`。失败会区分 `GIT_PUSH_AUTH_FAILED`、`GIT_PUSH_NETWORK_FAILED`、`GIT_PUSH_NON_FAST_FORWARD`、`GIT_PUSH_REMOTE_REJECTED` 与通用 `GIT_PUSH_FAILED`。工具只允许把已存在的本地 branch 推送到预配置 remote 的同名 branch；不接收 URL、任意 refspec、`--force` 或其他 Git 参数。`remote.<name>.pushurl`、repo-local `url.*` rewrite、`file://`、`git://`、明文 `http://` 和本地路径 remote 会被拒绝。HTTPS 使用服务账户已有 Git credential helper；SSH 使用服务账户 HOME/SSH agent，但这些凭据不会暴露给普通 `run_command`。`run_command git push` 仍然被禁止。

错误恢复见 [GIT_RECOVERY.md](GIT_RECOVERY.md)。

## OAuth scopes

| 工具 | OAuth scopes |
|---|---|
| `list_workspaces` / `list_projects` / `inspect_project` / `continue_task` / task checkpoint 查询 | `projects:read` |
| 文件读取/搜索/目录、Git 查询 | `files:read` |
| 文件写入/patch/创建目录 | `files:write` |
| 命令/Shell | `commands:execute` + `files:write` |
| Job 查询/取消 | `commands:execute` |
| Git commit | `git:write` + `files:write` |
| Git push | `git:write` |

这些 scope 属于同一个 owner；当前不提供多用户或每 OAuth 客户端独立 Project 列表。
