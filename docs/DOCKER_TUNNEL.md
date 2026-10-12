# cloudflared 容器访问宿主机

针对已确认的部署：cloudflared 在 Docker，EndlessVibe 在同一宿主机以普通用户运行。网络配置已有成功结果时，无需重建隧道。

Rust 默认监听 `0.0.0.0:20000`，实际工具执行必须认证，静态协议/工具发现公开。Cloudflare 对该域名的 Service URL 继续为 `http://host.docker.internal:20000`，Type 为 HTTP，Path 留空，不能用 `https://127.0.0.1:20000` 回源。

仅在原容器尚无宿主机映射时，在既有 Compose 服务合并：

```yaml
services:
  cloudflared:
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

包内 `deploy/cloudflared.override.example.yaml` 是合并片段，不是独立的完整 Compose。保留自己的 image、token、command、其他域名、mount 和 networks。服务名不是 cloudflared 时替换为实际名称。变更容器配置要通过原 Compose 重新创建，对现有连接有短暂中断；普通 restart 不会应用新 extra_hosts。

```bash
docker compose -f compose.yaml -f cloudflared.override.yaml up -d --no-deps --force-recreate cloudflared
```

上述文件名为示例，不能在不确认原文件时执行。原来使用 docker run 的部署，在自己的原创建命令加入 `--add-host=host.docker.internal:host-gateway` 后重建，而非替换所有部署参数。

## 验证

```bash
curl -q --noproxy '*' -i http://127.0.0.1:20000/health
docker run --rm --add-host=host.docker.internal:host-gateway curlimages/curl -q --noproxy '*' -i --max-time 5 http://host.docker.internal:20000/health
curl -i https://mcp.example.com/.well-known/oauth-protected-resource
```

临时 curl 容器测试的是默认桥接网络；自定义网络需要显式添加 `--network 实际名称`。这不会更改已有容器，也不需要 project 挂载。

主页工作不能证明完整 MCP/OAuth 工作。新版域名必须同时覆盖 `/assets/*`、`/.well-known/*`、`/oauth/*` 和 `/mcp`。Cloudflare 502 查看实际 cloudflared 日志；401 是本服务认证，不能误判为 Tunnel 故障。

`0.0.0.0` 是监听地址而不是回源目标地址。不要无条件开放整个防火墙；按实际容器网段控制宿主机 20000 端口。容器位于另一台机器、rootless 特殊网络或隔离 namespace 时，host-gateway 的具体可达性需在那一环境验证。

若未来把 EndlessVibe 也容器化，两者加入同一 user-defined Docker 网络后可用服务名寻址，并只把项目挂载给 EndlessVibe。不要为了嵌套沙箱随意添加 `--privileged`、挂载 Docker socket 或关闭安全限制；这一部署不是本包实测范围。
