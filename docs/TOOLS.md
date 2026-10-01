# MCP 工具参数与工作流

接口由 `src/tools/types.rs` 的 Rust 类型生成 JSON Schema。本文参数是示例，不代表已经执行。所有工具都需要有效 OAuth Bearer；工作区权限还要单独允许。

## 项目与文件

```json
{"workspace":"BAfter"}
```

供 `inspect_project`、`git_status` 使用。项目路径只来源于本机配置，先调用无参数的 `list_projects` 获得 ID。

```json
{"workspace":"BAfter","path":"CMakeLists.txt","start_line":1,"max_lines":200}
```

`read_file` 返回完整文件 SHA-256、实际返回内容、行数和 `next_line`。返回分页内容的 hash 仍对应完整文件。默认单文件 2 MiB、单次读取 256 KiB。

```json
{"workspace":"BAfter","path":"src","offset":0,"limit":200}
```

`list_directory` 返回 `next_offset`；遍历时检查分页，不把一页当成全部。符号链接标记 `symlink_blocked`，不可跟随。

```json
{"workspace":"BAfter","path":".","query":"FormInfoPower","regex":false,"case_sensitive":true,"max_results":100}
```

`search_code` 使用 Rust regex 或转义后的字面匹配，不运行 shell/rg；跳过常见构建目录、敏感名称和二进制。检查 `truncated`、`skipped`，不能声称搜索覆盖了被跳过内容。

```json
{"workspace":"BAfter","path":"docs/new.md","content":"# New document\n","expected_sha256":"MISSING","create_parents":true}
```

`write_file` 创建新文件使用 `MISSING`。已有文件必须使用 `read_file.sha256`，不得伪造。旧内容保存到 `state/backups/<backup_id>`，不是公开可下载接口。备份只保留字节，没有自动恢复命令；本机恢复前应重新检查当前文件，避免覆盖新的用户修改。

```json
{"workspace":"BAfter","path":"docs/new.md","expected_sha256":"从 read_file 返回的真实摘要","edits":[{"old_text":"New document","new_text":"Updated document","expected_occurrences":1}]}
```

`apply_patch` 顺序应用精确替换，**不接收统一 diff**。匹配次数不符、文件变化或输出大小超限则失败。`create_directory` 参数为工作区与相对目录路径，会创建缺失父目录。

## 命令与 Shell

```json
{"workspace":"mountain_and_sea","program":"cargo","args":["check"],"cwd":".","request_id":"mas-check-001","timeout_seconds":120}
```

`run_command` 不拼接 shell 字符串。返回 `job_id` 并不代表构建完成；它可能还在 queued 状态或随后因沙箱/工具链失败。必须查询实际结果。

```json
{"workspace":"BAfter","script":"printf '%s\n' 'hello'; pwd","cwd":".","request_id":"shell-001","timeout_seconds":10}
```

`run_shell` 必须由管理员设置 `execution.allow_shell=true`。通过 Bash `--noprofile --norc -c` 执行，默认仍处于 bubblewrap 沙箱。任意 Shell 或通用语言解释器意味着广泛执行权限，不能依靠程序名白名单当成安全沙箱。

```json
{"job_id":"提交任务时返回的真实 ID"}
```

供 `get_job` 和 `cancel_job` 使用。`cancel_job` 是取消请求；继续轮询直到 `cancelled`/其他终态。终态为 `succeeded`、`failed`、`timed_out`、`cancelled`、`interrupted`，看 `exit_code` 和 `error`，不要只看 HTTP 成功。

```json
{"job_id":"真实 ID","offset":0,"limit":65536}
```

`get_job_output` 返回 stdout/stderr 带流标记的混合输出、`next_offset`、`dropped_before`。游标按原始 UTF-8 字节计算，分页跨字符时显示替代字符是已知限制，不是 byte offset 可用字符数替代。达到输出容量后保留最新部分，不无限增长。

```json
{"workspace":"BAfter","limit":20}
```

`list_jobs` 可省略工作区。默认保留最近 100 个已完成任务的记录/输出；幂等记录保留 7 天。输出已过保留期但仍在幂等窗口内的请求返回 expired，不擅自重跑。服务停止/重启不会自动重新执行旧命令。

## Git

```json
{"workspace":"BAfter","paths":["src/example.cpp","README.md"]}
```

`git_diff` 生成这组文件的工作树快照相对 HEAD 的差异（包括新文件或删除文件）。传空列表时可选择当前安全路径的变更，但实际 commit 必须使用返回的明确列表。返回 `head`、`paths`、`diff_sha256`。删除能力不通过文件工具暴露，但可以审查/提交你已经自行删除的受控文件。

```json
{"workspace":"BAfter","paths":["README.md","src/example.cpp"],"message":"fix(example): handle missing value","expected_head":"git_diff 返回的 HEAD","expected_diff_sha256":"git_diff 返回的审查摘要"}
```

`git_commit` 不等于 `git add . && git commit`：只提交审查文件，选中文件已有用户暂存修改时拒绝，其他文件的暂存内容保留；不改写工作树、不运行 hooks/filters/签名、不推送。摘要包含 HEAD、精确路径和 diff，不是对 diff 字符串单独做 hash。首次提交 HEAD 使用字面量 `UNBORN`。

提交报错后先查看 `git_status`、真实 HEAD 和恢复日志，不能盲目重发、reset 或删除 index.lock。见 `GIT_RECOVERY.md`。

## 权限对应

| 工具 | OAuth scopes |
|---|---|
| 项目列表/检查 | `projects:read` |
| 读取/搜索/目录、Git 查询 | `files:read` |
| 文件写入/patch/创建目录 | `files:write` |
| 命令/Shell | `commands:execute` + `files:write` |
| 任务查询/取消 | `commands:execute` |
| Git 提交 | `git:write` + `files:write` |

这些 scope 都属于同一个管理员。并未实现按客户端区分项目列表或多个用户身份。
