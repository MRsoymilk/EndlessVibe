# 技术来源与核对边界

本项目是基于先前 EndlessVibe 0.2.0 源码的扩展，不是未经核实复制 CodexPro 实现。以下为编写本版时使用的官方/上游参考；文档页会变化，依赖版本以 Cargo.toml 和目标机器生成的 Cargo.lock 为准。

- OpenAI MCP OAuth、PKCE、issuer identification、动态客户端注册与ChatGPT回调：https://developers.openai.com/plugins/build/auth
- OpenAI MCP 服务端及客户端连接开发：https://developers.openai.com/plugins/build/mcp-server
- Rust MCP 官方 SDK 项目：https://github.com/modelcontextprotocol/rust-sdk
- Rust SDK tool 宏元数据源码：https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/main/crates/rmcp-macros/src/tool.rs
- MCP 生命周期及Streamable HTTP：https://modelcontextprotocol.io/specification/2025-06-18/basic/transports
- Docker host-gateway / extra host：https://docs.docker.com/reference/cli/docker/container/run/
- bubblewrap 上游威胁边界与用法：https://github.com/containers/bubblewrap
- Rust 子进程与 argv：https://doc.rust-lang.org/std/process/struct.Command.html
- Cargo 构建脚本（编译等于允许执行项目代码）：https://doc.rust-lang.org/cargo/reference/build-scripts.html
- Git index 和提交工具参考：https://git-scm.com/docs/git-update-index 、https://git-scm.com/docs/git-commit-tree 、https://git-scm.com/docs/git-update-ref

rmcp 3.5.0 沿用此前用户已连接的源码基线。生成环境无法解析/下载 Cargo crates，也没有 Rust 编译器，所以通过上游 API 文档核对不等于成功编译；真实 SDK 类型/宏、运行时及 ChatGPT 对接以目标环境验证为准。
