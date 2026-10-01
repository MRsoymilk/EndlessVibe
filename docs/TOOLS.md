# MCP 工具参数与工作流

接口由 `src/tools/types.rs` 的 Rust 类型生成 JSON Schema。所有私有工具都需要有效 OAuth Bearer；Project 的写入、执行和 Git 权限还需由本地配置允许。

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
{"workspace":"projects","project":"mountain_and_sea","program":"cargo","args":["check"],"cwd":".","request_id":"mas-check-001","timeout_seconds":120,"task_id":"combat-refactor","stage":"validate-core"}
```

任务立即返回 `job_id`。请求去重键是 `workspace + project + request_id`，因此不同 Project 可以使用相同 request_id 而不会互相复用。`task_id` 与 `stage` 必须同时提供或同时省略；提供后 Job 会自动挂到对应阶段 checkpoint。

`run_shell`：

```json
{"workspace":"projects","project":"BAfter","script":"printf '%s\n' 'hello'; pwd","cwd":".","request_id":"shell-001","timeout_seconds":10}
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
{"job_id":"真实 Job ID","offset":0,"limit":65536}
```

`list_jobs` 可以按 Workspace、Project、`task_id` 过滤：

```json
{"workspace":"projects","project":"BAfter","task_id":"release-031","limit":20}
```

终态包括 `succeeded`、`failed`、`timed_out`、`cancelled`、`interrupted`。

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

stream 中断或新会话恢复时：

`get_task_checkpoint`：

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
{"workspace":"projects","project":"BAfter","paths":["src/example.cpp","README.md"]}
```

返回 `head`、明确的 `paths`、`diff` 和 `diff_sha256`。

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
| `list_workspaces` / `list_projects` / `inspect_project` / task checkpoint 查询 | `projects:read` |
| 文件读取/搜索/目录、Git 查询 | `files:read` |
| 文件写入/patch/创建目录 | `files:write` |
| 命令/Shell | `commands:execute` + `files:write` |
| Job 查询/取消 | `commands:execute` |
| Git commit | `git:write` + `files:write` |
| Git push | `git:write` |

这些 scope 属于同一个 owner；当前不提供多用户或每 OAuth 客户端独立 Project 列表。
