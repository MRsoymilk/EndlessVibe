# 单管理员 OAuth

本服务实现一个小型、内置、单管理员的授权码授权服务器，供已有自托管 MCP 通过 HTTPS 与 ChatGPT 连接。**没有独立安全审计；不是 OIDC 身份平台，也不声称完整实现全部 OAuth 扩展。** 多人/生产部署优先使用成熟 IdP，并在独立审计后接入。

## 端点与契约

| 路径 | 行为 |
|---|---|
| `/.well-known/oauth-protected-resource` | 资源为规范公网 URL 加 `/mcp` |
| `/.well-known/oauth-protected-resource/mcp` | 相同资源发现文档 |
| `/.well-known/oauth-authorization-server` | issuer、S256、DCR、授权/Token/撤销端点 |
| `POST /oauth/register` | 动态注册，默认只允许精确的 ChatGPT 回调格式 |
| `GET /oauth/authorize` | 检查 client、redirect、resource、PKCE、state 后显示授权页 |
| `POST /oauth/authorize` | 检查一次性会话、Cookie、管理员 key，确认或拒绝 |
| `POST /oauth/token` | authorization_code 或 refresh_token；Form 编码 |
| `POST /oauth/revoke` | 客户端验证后撤销对应 Token family |

授权码、access/refresh tokens 为 OS 随机值。数据库只保存其摘要；scope、client、resource 和过期时间绑定到令牌。`resource` 必须与 metadata 完全相同，不从 Host/X-Forwarded-Host 临时推断，不接受 Token passthrough 或 URL query token。

授权页的管理员 key **不是 OAuth Client Secret，也不是 MCP access token**。初始化生成的 256-bit key 仅在自己的授权页面输入；本机诊断通过 `--issue-token` 签发独立短期 token。不会把 key 放入页面源码、状态 JSON、审计内容或 stdout 日志。

DCR 支持 `none`、`client_secret_post` 和 `client_secret_basic`，默认为 public client / none。未实现 CIMD，不在 metadata 宣称 CIMD 支持。对于选择 DCR 的 ChatGPT 连接无需自行硬编码 client_id/client_secret；若界面显示的配置方式不同，以客户端实际管理页为准，不把本机 key 填进那些字段。

## HTTPS 与回调

`server.public_url` 是规范 issuer 和网页 origin，只填根地址，不填 `/mcp` 或 query。默认使用用户已配置的公网域名；公网改变后必须修改配置、重启、撤销旧 Token 并重新授权。

默认回调包括 `https://chatgpt.com/connector_platform_oauth_redirect` 和具有受限 callback ID 的 `https://chatgpt.com/connector/oauth/{id}`。DCR 注册后授权请求仍须与登记字符串完全相同。成功和用户拒绝的重定向均带原始 state 和精确 issuer `iss`。

额外客户端（例如 Inspector）回调添加到 `security.extra_redirect_uris`，不要登记通配符。只允许 HTTPS；调试 HTTP loopback 需要同时启用 `security.allow_http_loopback=true`，且 URI 为 localhost/回环地址。生产域名必须 HTTPS。配置文件不会进行 `~`/环境变量展开。

授权页 Cookie 为 HttpOnly、SameSite=Lax、短期一次性会话，公网场景为 Secure。请求 Cookie、CSRF 状态和可选 Origin 必须匹配。初始页面使用 CSP，并仅允许 self、ChatGPT 和额外登记回调 origin 作为表单跳转目标，避免浏览器阻止合法 OAuth 303。

授权 HTML 响应使用 `Referrer-Policy: same-origin`，其他响应仍默认 `no-referrer`。浏览器表单 POST 在 `no-referrer` 策略下可能发送 `Origin: null`，会被同源检查拒绝；同源策略保留正确 Origin，且不向跨站回调发送页面 Referer。同源校验使用 `server.public_url` 的规范化 origin，不受主机名大小写或显式默认端口影响。

## 撤销与轮换

```bash
./target/release/endlessvibe --revoke-all
./target/release/endlessvibe --audit-tail
```

第一条撤销所有 access/refresh、待处理授权和授权码，不删除已登记客户端；不会修改项目。刷新 Token 实施轮换，再次使用已轮换的 refresh token 会撤销其 family，不能在刷新失败后无限重试旧值。

更换管理员 key 需要**先停止服务**，然后运行：

```bash
./target/release/endlessvibe --rotate-owner-key
```

这会撤销旧 Token 并替换 key。重新启动并重新连接 OAuth。配置和状态应一起备份；状态包含 Token 摘要、私有输出等敏感数据，即使没有明文 access token 也不能公开。

## 排错

若提交管理员密钥后出现 `invalid_request / Cross-origin consent POST refused`，检查授权 HTML 的响应头是否为 `Referrer-Policy: same-origin`。修复需要重启新构建的服务，并从 ChatGPT 重新打开授权页；已打开的旧页面仍使用旧策略。日志 `Consent origin check failed origin_check=opaque` 表示请求携带 `Origin: null`，`mismatch` 表示来源与配置的公网 origin 不同。若新页面仍出现 `mismatch`，确认地址栏域名与 `server.public_url` 一致；同源检查不会信任代理头或放行 `null`。

匿名 `GET /mcp` 或非公开方法返回 `401` 和资源发现 header 是正常行为。静态 `initialize`、`notifications/initialized`、`ping` 和 `tools/list` 可在授权前访问。未授权、Token 过期或权限不足的 `tools/call` 返回 HTTP `200` 的 JSON-RPC 工具错误：`result.isError=true`，并在 `result._meta["mcp/www_authenticate"]` 携带认证挑战。`invalid_token` 表示需要连接/重连，`insufficient_scope` 表示需要补充工具声明的权限；这些错误不会执行工具。

遇到 `invalid_redirect_uri`，只在本机确认管理页显示的真实回调后精确登记，不能为排错允许任意 callback。Token 失败时检查 resource、PKCE、issuer 和过期时间，不把 verifier、code、Token 或 owner key 发到聊天。

Cloudflare 必须转发整个域名，不能仅匹配 `/mcp`；OAuth 路径不可被交互式 Cloudflare Access 页面或缓存规则替换。官方认证参考见 `SOURCES.md`。本次生成包未实测 ChatGPT v0.3 OAuth 登录；必须完成真实浏览器验收。

### ChatGPT 提示 expired / 没有 browser setup URL

如果首次连接从未打开过授权页，`Your connection has expired` 并不能证明 access token 真正过期。`This app does not provide a browser setup URL right now` 表示客户端当前无法启动浏览器授权；应先检查发现、注册和连接记录，而不是延长令牌有效期。

代码已补齐 ChatGPT 工具级授权契约：每个工具同时提供顶层 `securitySchemes` 与 `_meta.securitySchemes`，认证错误提供 `_meta["mcp/www_authenticate"]`（包含 `resource_metadata`、`error`、`error_description` 和 scope）。SDK 缺少顶层扩展字段，因此在 HTTP 工具列表序列化时补充。参见 [OpenAI 授权 UI 触发要求](https://developers.openai.com/plugins/build/auth#triggering-authentication-ui)。

部署修复后，先停止原服务进程，再启动新构建的 `target/release/endlessvibe`，然后刷新 ChatGPT 中的工具元数据并重新连接；若连接记录仍没有授权入口，移除失败记录后用 OAuth/DCR 重新创建。只重新编译不会更新已运行的进程。正常初始化与工具发现应返回 `200`，调用未授权工具应返回带认证挑战的工具错误，随后客户端进行发现、注册、授权页跳转和 Token 交换。

若重连仍失败，日志中的 `MCP request mcp_method=... authentication=missing/invalid/valid` 可以确认 `/mcp` 请求属于哪一阶段。`OAuth request rejected oauth_error=...` 提供错误码，不包含凭据；仅看到 `/mcp` 请求不能证明客户端已经完成 OAuth 发现或动态注册。

2026-10-01 对公网域名的诊断发现：`curl` 请求发现文档返回 `200`，DCR 返回 `201`，授权页返回 `200`；但 Python 默认 HTTP 客户端访问两个发现文档、`/mcp` 和 DCR 时，Cloudflare 返回 `403`，正文为 `error code: 1010`。所以浏览器或 `curl` 能访问不能证明其他 OAuth 客户端也能访问。这个结果确认了按客户端特征拦截的问题；ChatGPT 是否命中同一规则需结合 Cloudflare 安全事件确认。

Cloudflare 官方说明：1010 是基于浏览器签名的拒绝，Browser Integrity Check 会拦截或挑战非标准 User-Agent。修复方法：

1. 在 Cloudflare 的 `soymilk.xin` 站点创建配置规则，匹配：
   ```text
   http.host eq "endlessvibe.soymilk.xin"
   ```
2. 将该规则的 **Browser Integrity Check（浏览器完整性检查）设为关闭**。也可使用自定义规则的 Skip 动作，仅跳过 Browser Integrity Check。保留 EndlessVibe 自身 OAuth 验证。
3. 再用非浏览器客户端请求发现文档，确认返回 JSON `200`，不再返回 `403 / 1010`。匿名 `GET /mcp` 应返回应用自身的 `401` 和 `WWW-Authenticate`；静态工具发现和工具级认证错误按上文处理。
4. 在 ChatGPT 应用/连接管理页刷新元数据并重新连接。如果仍没有浏览器入口，移除失败的连接后重新创建，URL 使用完整 `https://endlessvibe.soymilk.xin/mcp`，认证选择 OAuth 和自动注册/DCR，Client ID/Secret 留空。
5. 完成自有域名上的管理员密钥授权后，新建对话测试 `hello` 和 `list_projects`。

若仍失败，记录创建/重连时的服务端 HTTP 路径与状态码，并在 Cloudflare 安全事件中检查对应请求。被 Cloudflare 拦截的请求不会到达 Rust 服务。不要发送 URL 查询参数、Cookie、授权码或令牌。

官方说明：[Error 1010](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1010/)、[Browser Integrity Check 与按规则关闭](https://developers.cloudflare.com/waf/tools/browser-integrity-check/)。
