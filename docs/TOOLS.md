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
{"workspace":"projects","project":"mountain_and_sea","program":"cargo","args":["check"],"cwd":".","request_id":"mas-check-001","timeout_seconds":120}
```

任务立即返回 `job_id`。请求去重键是 `workspace + project + request_id`，因此不同 Project 可以使用相同 request_id 而不会互相复用。

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

`list_jobs` 可以按 Workspace、Project 或两者过滤：

```json
{"workspace":"projects","project":"BAfter","limit":20}
```

终态包括 `succeeded`、`failed`、`timed_out`、`cancelled`、`interrupted`。

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
{"workspace":"projects","project":"BAfter","paths":["README.md","src/example.cpp"],"message":"fix(example): handle missing value","expected_head":"git_diff 返回的 HEAD","expected_diff_sha256":"git_diff 返回的审查摘要"}
```

`git_commit` 只提交已审查文件；选中文件已有用户 staged 修改时拒绝，其他 staged 内容保留。不运行 hooks/filters/签名，不 push，不改写工作树。首次提交的 HEAD 使用 `UNBORN`。

错误恢复见 [GIT_RECOVERY.md](GIT_RECOVERY.md)。

## OAuth scopes

| 工具 | OAuth scopes |
|---|---|
| `list_workspaces` / `list_projects` / `inspect_project` | `projects:read` |
| 文件读取/搜索/目录、Git 查询 | `files:read` |
| 文件写入/patch/创建目录 | `files:write` |
| 命令/Shell | `commands:execute` + `files:write` |
| Job 查询/取消 | `commands:execute` |
| Git commit | `git:write` + `files:write` |

这些 scope 属于同一个 owner；当前不提供多用户或每 OAuth 客户端独立 Project 列表。
