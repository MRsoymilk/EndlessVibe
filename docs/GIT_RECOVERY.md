# Git 提交范围与恢复

## 提交算法

`git_diff` 用私有临时 index 和临时 object directory 构造选中文件快照，不修改真实暂存区，不把新预览 blob 写进原仓库。review 包含精确 HEAD、排序后的文件路径和完整 diff。

`git_commit` 获得工作区服务锁以及 Git 标准 `index.lock`，重新构造和校验 review，拒绝选中文件已经存在用户暂存修改的情况。只把已审查文件的 blob 写入仓库，并通过 `commit-tree` 创建提交对象。将选中的条目合并到真实 index 的副本，保留不相关条目的暂存状态。

随后写入私有恢复日志、用 `update-ref` 比较并交换分支 HEAD，并原子发布 index.lock。既不 `git add .`，也不 reset/checkout/clean/push；工作树文件不改写。外部编辑器在快照之后产生的新内容会留在工作树中，不偷偷被回退。

## 限制

只支持 `.git` 为真实目录的普通工作区；linked worktree、bare repo、submodule 作为选中文件、符号链接/硬链接文件、外部对象 alternates、部分克隆、sparse、包括 includes/filter/LFS/diff helper 的配置在此版本拒绝。仓库配置先复制到私有文件，用只解析配置的 Git 命令在 `--no-includes` 模式检查，不为“兼容”而改写原始 `.git/config`。有相关配置的项目需要人工审查，不会自动禁用/删除用户功能。

使用 `hash-object --no-filters` 读取原始文件字节，禁用 hooks、fsmonitor、签名、外部 diff/textconv、lazy fetch 和远端协议；没有 LFS 转换。author/committer 由 `[git]` 配置，不读取/覆盖全局 Git 用户配置。初次提交支持 UNBORN；detached HEAD 拒绝提交。最大 64 个文件、16 MiB 总读取，diff 还受输出上限约束。

Git 元数据最终由宿主机 Git 访问。配置检查、锁和限制不能对抗同时以相同 OS UID 恶意替换仓库元数据的其他进程，不能把它作为不可信任意仓库的完整安全沙箱。应只登记可信本机仓库。

## 错误的处理原则

| 错误 | 处理 |
|---|---|
| `GIT_BUSY` | 先查真实 Git 进程；不要自动删除用户已有 index.lock |
| `GIT_CONFLICT` | HEAD/文件/路径审查不匹配，重新读取与 review |
| `STAGED_CONFLICT` | 用户先自行处理选中文件暂存状态；工具不会替用户 reset |
| `GIT_UPDATE_AMBIGUOUS` | 保留 index.lock 和私有 journal，先调查真实 HEAD |
| `COMMIT_PARTIALLY_PUBLISHED` | HEAD 可能已完成，而 index 发布失败；不能直接重试提交 |

HTTP 超时不意味着 Git 一定没有提交。Git 事务脱离单次 HTTP 请求的取消生命周期继续完成；必须根据真实 HEAD、日志和返回的 commit 判断。终止进程/断电时仍可能落在 ref/index 两步之间，所以实现了恢复 journal，而不是宣称文件系统能跨多个 Git 文件提供全局原子事务。

## 手工恢复

先停止服务，确认没有其他 Git 操作，不要删除整个 `.git`、reset、clean 或 force push。私有状态目录 `git-journal/*.json` 记录 workspace、branch、old/new HEAD、index.lock 路径和 SHA-256。

人工分别验证当前分支 HEAD、日志中的 new_head、index.lock 的摘要，以及现有工作树/index 状态。只有确认 HEAD 已等于记录 new_head，且 index.lock 精确匹配预期摘要，才能考虑把该锁文件原子发布为 index。若 HEAD 仍为 old_head、已经被其他操作推进、锁文件不是本次创建，或状态不确定，保留副本并人工处理，不能套用通用删除命令。

服务不会自动恢复 Git 元数据或覆盖用户工作。未完成提交产生的无引用对象可由 Git 自己的常规维护之后处理，本工具不擅自执行 gc。
