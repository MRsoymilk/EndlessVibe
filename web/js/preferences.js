/*
 * EndlessVibe UI localization.
 * The Chinese copy already exists in the app. We translate presentation text only,
 * never code blocks, tool output, project paths, secrets, or user input values.
 */
const zhToEn = new Map(Object.entries({
  // Local setup and ChatGPT connection guide.
  "连接向导":"Connection setup",
  "将本机项目连接到 ChatGPT":"Connect local projects to ChatGPT",
  "按照向导设置域名、连接 ChatGPT，再选择允许访问的项目。":"Follow the guide to set a domain, connect ChatGPT, and choose which projects it may access.",
  "打开连接向导":"Open connection setup",
  "连接 ChatGPT":"Connect ChatGPT",
  "从本机开始：设置公网域名、重启服务，再连接并授权项目。以后可随时回到此向导修改设置。":"Start on this computer: set a public domain, restart the service, then connect and authorize projects. Return here whenever you need to change these settings.",
  "检查当前配置":"Check current configuration",
  "本机连接状态":"Local connection status",
  "已保存地址":"Saved address",
  "监听地址":"Listen address",
  "当前运行地址":"Active address",
  "正在读取连接设置…":"Loading connection settings…",
  "本机配置位置":"Local configuration location",
  "连接向导仅在本机 Dashboard 提供。公网隧道应指向 MCP 服务端口，不能指向 Dashboard。":"This guide is available only on the local Dashboard. Point the public tunnel at the MCP service port, never at the Dashboard.",
  "启动参数可能覆盖网页设置":"Startup options may override these settings",
  "检测到启动环境变量：":"Startup environment variables detected:",
  "请修改原来的启动环境后再重启，同时检查启动命令中的":"Update the original startup environment before restarting. Also check the startup options",
  "和":"and",
  "。以这里的当前运行地址为准。":". Use the active address shown here to confirm the result.",
  "设置公网域名":"Set a public domain",
  "先让隧道或反向代理提供 HTTPS 地址，再填写到这里。":"First obtain an HTTPS address from your tunnel or reverse proxy, then enter it here.",
  "公网域名或 HTTPS 根地址":"Public domain or HTTPS origin",
  "填写你的域名，不含 /mcp":"Enter your domain without /mcp",
  "支持自己的域名、Cloudflare Tunnel 或 ngrok 分配的域名。填写根地址即可，程序会生成 /mcp 连接地址。":"Use your own domain or one provided by Cloudflare Tunnel or ngrok. Enter only the origin; EndlessVibe generates the /mcp address.",
  "隧道运行位置":"Where the tunnel runs",
  "本机隧道":"Local tunnel",
  "隧道与 EndlessVibe 在同一台主机运行":"The tunnel and EndlessVibe run on the same host",
  "Docker 中的代理":"Proxy in Docker",
  "代理在容器中，EndlessVibe 在宿主机运行":"The proxy runs in a container; EndlessVibe runs on the host",
  "MCP 地址预览":"MCP address preview",
  "填写域名后生成":"Generated after you enter a domain",
  "代理上游地址":"Proxy upstream address",
  "保存后的 MCP 监听地址":"MCP listen address after saving",
  "本机隧道连接 127.0.0.1。将域名转发到上面的代理上游地址，并保留完整请求路径。":"A local tunnel connects to 127.0.0.1. Forward the domain to the upstream address above and preserve the full request path.",
  "Docker 容器中的 127.0.0.1 指向容器自身。请使用宿主机地址；Linux Docker 可能还需配置 host.docker.internal 的 host-gateway 映射，或使用容器可达的实际宿主机地址。此选项只调整 MCP 监听地址，Dashboard 仍仅供本机访问。":"Inside Docker, 127.0.0.1 refers to the container. Use the host address. On Linux, configure a host-gateway mapping for host.docker.internal or use a host address reachable from the container. This changes only the MCP listener; the Dashboard remains local.",
  "使用 ngrok 的示例":"Example using ngrok",
  "按 ngrok 官方指引在本机终端配置账号和 authtoken，然后运行以下命令。无需把 authtoken 填入本网页。":"Follow the ngrok guide to configure your account and authtoken in a local terminal, then run the command below. Do not enter the authtoken on this page.",
  "填写公网域名后显示 ngrok 命令。":"Enter a public domain to see the ngrok command.",
  "此命令用于在宿主机运行的 ngrok；目标是 MCP 端口":"This command runs ngrok on the host and targets MCP port",
  "，不是 Dashboard 端口。":", not the Dashboard port.",
  "ngrok 官方指南 ↗":"Official ngrok guide ↗",
  "保存连接设置":"Save connection settings",
  "其他操作已更新配置。下面的按钮会用最新已保存配置替换当前连接输入，请核对后重新编辑。":"Another operation updated the configuration. The button below replaces your connection inputs with the latest saved settings. Review them before editing again.",
  "加载最新已保存配置":"Load latest saved settings",
  "重启并核对配置":"Restart and check the configuration",
  "保存后的域名与监听设置，需要重启才能生效。":"Saved domain and listener changes take effect after a restart.",
  "正在读取重启提示…":"Loading restart instructions…",
  "在本机终端执行：":"Run in a local terminal:",
  "请先等待当前任务结束，再通过原来的启动方式重启服务。重新连接此网页后，检查“已保存地址”和“当前运行地址”是否一致。":"Wait for active tasks to finish, then restart using your existing service setup. Reopen this page and compare the saved and active addresses.",
  "我已重启，检查状态":"I restarted; check status",
  "重新进行初始化":"Run setup again",
  "在本机终端执行以下命令，可再次打开连接配置入口：":"Run this command locally to return to connection setup:",
  "已有配置会保留。你也可以直接访问本机的 /setup 页面。":"Existing settings are preserved. You can also visit the local /setup page directly.",
  "在 ChatGPT 中创建连接":"Create a connection in ChatGPT",
  "使用 OAuth 授权，无需 OpenAI API Key。":"Authorize with OAuth. No OpenAI API key is needed.",
  "已保存的 MCP 地址":"Saved MCP address",
  "先保存公网域名":"Save a public domain first",
  "复制 MCP 地址":"Copy MCP address",
  "保存公网域名后，这里会生成 MCP 地址。":"The MCP address appears here after you save a public domain.",
  "打开 ChatGPT 插件页面，点击“+”，选择“Add custom MCP server”。":"Open ChatGPT Plugins, click “+”, and select “Add custom MCP server”.",
  "打开 ChatGPT ↗":"Open ChatGPT ↗",
  "填写名称和描述，粘贴上面的 HTTPS MCP 地址，选择 OAuth，使用“Create as a plugin”创建插件。":"Enter a name and description, paste the HTTPS MCP address above, choose OAuth, and select “Create as a plugin”.",
  "按连接页面提示完成 EndlessVibe 授权，检查工具后安装插件。在新会话中使用 @ 选择 EndlessVibe。":"Complete EndlessVibe authorization when prompted, review the tools, and install the plugin. Select EndlessVibe with @ in a new conversation.",
  "入口名称或可用性可能受账号和工作区策略影响。若页面要求填写 Redirect URI，使用该 ChatGPT 连接页面显示的完整地址。":"Labels and availability may depend on account and workspace policies. If a Redirect URI is requested, use the exact address shown on that ChatGPT connection page.",
  "授权时需要的 Owner Key 在哪里？":"Where is the Owner Key for authorization?",
  "在本机读取以下文件，并仅将其中的 Owner Key 输入 EndlessVibe 授权页面：":"Read this file locally and enter its Owner Key only on the EndlessVibe authorization page:",
  "Owner Key 用于确认本机所有者身份，不是 OpenAI API Key。本向导只显示文件位置，不读取密钥内容。":"The Owner Key confirms ownership of this computer; it is not an OpenAI API key. This guide shows only the file location and never reads the key.",
  "更换域名后，需要在 ChatGPT 更新 MCP 地址并重新授权。保存本页配置不会自动更新已有连接。":"After changing domains, update the MCP address in ChatGPT and authorize again. Saving here does not update an existing connection automatically.",
  "ChatGPT 连接官方指南 ↗":"Official ChatGPT connection guide ↗",
  "选择允许访问的项目":"Choose projects to authorize",
  "先添加管理根，再到 Projects 中选择具体项目和权限。":"Add a workspace root, then choose individual projects and permissions in Projects.",
  "管理根用于组织项目。添加管理根不会自动开放其中所有目录或文件，也不会自动授予写入和执行权限。":"A workspace root organizes projects. Adding one does not expose all its files or directories, and grants no write or command permissions automatically.",
  "管理根名称":"Workspace root name",
  "名称：字母、数字、下划线或短横线":"Name: letters, numbers, underscores, or hyphens",
  "已存在的绝对路径":"Existing absolute path",
  "填写本机项目根目录":"Enter a local project root directory",
  "填写已存在的绝对路径。":"Enter an existing absolute path.",
  "添加管理根":"Add workspace root",
  "刷新将保留上面的名称和路径，并读取最新配置，随后可重新提交。":"Refreshing keeps the name and path above while loading the latest configuration. You can then submit again.",
  "刷新配置后重试":"Refresh configuration to retry",
  "前往 Projects 添加授权项目":"Open Projects to authorize projects",
  "配置命令执行与沙箱 ↗":"Configure command execution and sandboxing ↗",
  "连接 ChatGPT / 初始化":"ChatGPT connection / Setup",
  "设置公网域名、查看隧道配置帮助和重启提示，或添加新的管理根。":"Set a public domain, read tunnel and restart instructions, or add a workspace root.",
  "连接配置已保存，等待重启":"Connection settings saved; restart needed",
  "打开连接向导，按提示重启并核对当前运行的地址。":"Open connection setup, follow the restart instructions, and check the active address.",
  "仅局域网":"LAN only",
  "尚未设置公网域名":"No public domain saved",
  "尚未启用公网连接":"Public connection not configured",
  "启动参数覆盖":"Startup override",
  "待重启":"Restart needed",
  "待配置":"Setup needed",
  "当前启动参数覆盖了已保存地址。请先调整原启动命令或环境变量，再重启并核对。":"Current startup options override the saved address. Update the original command or environment, then restart and check again.",
  "服务配置已更改。重启后，请确认“当前运行地址”与“已保存地址”一致。":"Service settings have changed. After restarting, confirm that the active and saved addresses match.",
  "服务已加载当前配置。公网是否可达，以及 ChatGPT 是否完成授权，需要在连接时确认。":"The service has loaded the current settings. Public reachability and ChatGPT authorization must still be confirmed when connecting.",
  "先准备一个公网 HTTPS 域名，再保存下方连接设置。":"Prepare a public HTTPS domain, then save the connection settings below.",
  "这是已保存的 MCP 地址。请先重启 EndlessVibe，再到 ChatGPT 完成连接。":"This is the saved MCP address. Restart EndlessVibe before completing the connection in ChatGPT.",
  "复制此地址到 ChatGPT。保存配置不代表公网已可达或 ChatGPT 已连接。":"Copy this address into ChatGPT. Saved settings do not confirm public reachability or a ChatGPT connection.",
  "请使用当前安装方式停止并重新启动 EndlessVibe。":"Stop and restart EndlessVibe using your current installation method.",
  "先在原终端按 Ctrl+C 或通过服务管理器停止，再运行：":"First stop the service with Ctrl+C in its terminal or with your service manager, then run:",
  "请先移除或更新覆盖网页设置的启动参数和环境变量，再通过原来的启动方式重启。":"Remove or update startup options and environment variables that override these settings, then restart using your existing service setup.",
  "当前有尚未生效的配置。请在运行 EndlessVibe 的本机终端或服务管理器中重启。":"Some saved settings are not active yet. Restart from the local terminal or service manager that runs EndlessVibe.",
  "当前配置无需重启。修改域名或连接方式并保存后，再按这里的提示操作。":"No restart is currently needed. Follow these instructions after saving domain or connection-mode changes.",
  "填写已存在的 Windows 绝对路径，例如 D:\\project。":"Enter an existing Windows absolute path, such as D:\\project.",
  "填写已存在的绝对路径，例如 /home/you/project。":"Enter an existing absolute path, such as /home/you/project.",
  "暂时无法读取配置。帮助内容仍可查看，服务恢复后请重新检查。":"Configuration is temporarily unavailable. You can still read the guide; check again when the service is back.",
  "请填写裸域名或 HTTPS 根地址；不要包含 /mcp、其他路径、账号、查询参数或片段。":"Enter a domain or HTTPS origin without /mcp, other paths, credentials, query parameters, or fragments.",
  "正在验证并保存连接设置…":"Validating and saving connection settings…",
  "连接设置已保存。请完成下方的重启步骤。":"Connection settings saved. Complete the restart step below.",
  "连接设置已保存，当前配置已生效。请继续到 ChatGPT 完成授权。":"Connection settings saved and active. Continue to ChatGPT to authorize the connection.",
  "配置已发生变化，当前输入已保留。请加载最新已保存配置后重新编辑。":"Configuration changed; your inputs are preserved. Load the latest saved settings before editing again.",
  "未能保存连接设置。":"Could not save connection settings.",
  "保存结果尚未确认。请检查当前配置后再决定是否重试。":"The save result is unconfirmed. Check the current configuration before deciding whether to retry.",
  "请填写管理根名称和已存在的绝对路径。":"Enter a workspace root name and an existing absolute path.",
  "正在添加管理根…":"Adding workspace root…",
  "管理根已保存，重启后生效。接下来可在 Projects 中添加授权项目。":"Workspace root saved; a restart is required. Next, add authorized projects in Projects.",
  "管理根已添加。接下来在 Projects 中选择具体项目并设置权限。":"Workspace root added. Next, choose individual projects and permissions in Projects.",
  "配置已发生变化，管理根输入已保留。请刷新配置后重试。":"Configuration changed; your workspace root inputs are preserved. Refresh the configuration before retrying.",
  "未能添加管理根。":"Could not add the workspace root.",
  "添加结果尚未确认。请在 Projects 核对已有管理根后再决定是否重试。":"The result is unconfirmed. Check existing workspace roots in Projects before deciding whether to retry.",
  "MCP 地址已复制。":"MCP address copied.",
  "浏览器未允许复制，地址已选中，请手动复制。":"The browser did not allow copying. The address is selected; copy it manually.",
  "连接设置有未保存的修改。":"Connection settings have unsaved changes.",
  "管理根信息尚未保存。":"Workspace root details have not been saved.",
  "启动参数仍在覆盖网页配置，请先调整启动命令或环境变量。":"Startup options still override the saved settings. Update the command or environment first.",
  "服务仍有待重启的配置，请核对启动方式和环境变量。":"The service still has pending settings. Check the startup method and environment variables.",
  "已重新读取当前运行配置。公网连接仍需在 ChatGPT 中验证。":"Active settings refreshed. The public connection still needs to be verified in ChatGPT.",
  "已加载最新已保存配置，请重新编辑后保存。":"Latest saved settings loaded. Edit them again before saving.",
  "已读取最新配置，管理根输入已保留，可以重新提交。":"Latest configuration loaded. Workspace root inputs are preserved; you can submit again.",
  "首页":"Home","活动":"Activity","日志":"Logs","配置":"Settings","沙箱":"Sandbox","任务":"Tasks","项目":"Projects",
  "节点":"Nodes","工具":"Tools","文件":"Files","命令":"Commands","连接":"Connection","其他":"Other",
  "保存":"Save","取消":"Cancel","刷新":"Refresh","删除":"Delete","查看":"View","详情":"Details",
  "深色":"Dark","浅色":"Light","主题":"Theme","语言":"Language","简体中文":"简体中文",
  "检查中":"Checking","运行中":"Running","离线":"Offline","启用":"Enabled","禁用":"Disabled",
  "服务运行正常":"Service running normally","正在检查服务":"Checking service","立即刷新":"Refresh now",
  "正在请求本地 Dashboard 状态接口。":"Requesting the local Dashboard status endpoint.",
  "尚未取得状态":"No status received yet","状态 JSON ↗":"Status JSON ↗",
  "服务版本":"Service version","运行时长":"Uptime","自本次服务启动":"Since this service start",
  "管理根节点":"Workspace roots","已授权项目":"Authorized projects","当前任务":"Current jobs",
  "执行后端":"Execution backend","运行信息":"Runtime information",
  "局域网发现、加密配对与远程 Project 管理。父节点发起请求，子节点核对六位验证码后选择同意或拒绝；无需双方重复批准。":"Discover, securely pair, and manage remote projects over the LAN. The parent sends a request; the child verifies the six-digit code and selects Accept or Reject. No second approval is required.",
  "父节点 · 发起连接申请":"Parent · Request pairing",
  "子节点 · 等待授权请求":"Child · Pending requests",
  "配对请求":"Pairing requests",
  "向子节点发起请求":"Send pairing request",
  "父节点 · Parent":"Parent",
  "子节点 · Child":"Child",
  "未配对 · 角色待确定":"Unpaired · Role unassigned",
  "角色冲突 · Mixed":"Role conflict · Mixed",
  "角色待确定":"Role unassigned",
  "同意":"Accept",
  "拒绝":"Reject",
  "等待扫描":"Waiting for scan",
  "输入子节点的局域网 IP:Transfer 端口，发出带验证码的配对请求。子节点同意后自动完成配对；Project 授权需之后单独设置。":"Enter the child's LAN IP and Transfer port to send a verification-code request. The parent pairs automatically after child approval; grant Project access separately.",
  "本机为子节点，不能主动发起父节点配对。收到请求后在下方查看来源与验证码，只需点击“同意”或“拒绝”。配对本身不授予 Project 权限。":"This node is a child and cannot start a parent pairing. Review the requesting parent and verification code, then click Accept or Reject. Pairing itself grants no Project access.",
  "尚未发现节点":"No nodes discovered",
  "没有等待确认的配对":"No pending pairings",
  "没有已配对节点":"No paired nodes",
  "没有发现其他节点。可手动输入局域网 IP:Port。":"No other nodes discovered. Enter a LAN IP:Port manually.",
  "没有待确认的配对":"No pending pairings",
  "尚无已配对节点":"No paired nodes",
  "仅展示当前父节点及授权 Project 的新请求记录。失败或中断不代表操作未生效，请先核查，禁止盲目重放。":"Only requests from the current parent and authorized Project are shown. Failed or interrupted requests may still have side effects; inspect before retrying.",
  "父节点将操作转发到配对子节点。子节点始终再次校验授权；断线后不要换 request_id 重复提交命令。":"The parent forwards operations to its paired child. The child rechecks authorization. Do not resubmit a disconnected command with a new request_id.",
  "管理 Workspace 下的 Project、权限与本地 Git 状态。":"Manage Workspace projects, permissions, and local Git status.",
  "检查配置状态":"Checking configuration",
  "添加 Project":"Add Project",
  "写入 config.toml；目标目录须已存在且位于所选 Workspace 内。支持 Windows、Linux 和 macOS 的绝对路径；实际工作空间地址仅展示当前配置，不会自动填入新项目路径。保存后尝试热重载；服务级配置变化可能需要重启。":"Writes to config.toml. The directory must exist inside the selected Workspace. Supports native Windows, Linux and macOS absolute paths. Existing Workspace paths are shown from the saved configuration and are never inserted into a new Project path input. Changes attempt a hot reload, but service-level settings may require a restart.",
  "trusted_host · 所有本机工具（高风险）":"trusted_host · all local tools (high risk)",
  "保存并添加":"Save and add",
  "Git 状态为本地只读快照；Project 正在执行操作时显示 busy。":"Git status is a local read-only snapshot. A Project currently in use is marked busy.",
  "多阶段 Task 关联 Jobs、Operations 和 Git 恢复点。未归属的执行自动记录为独立 Auto Job，不会自动混入其他会话的 Task；只有 Checkpointed 代表可恢复的 Git 提交。":"Multi-stage Tasks link Jobs, Operations, and Git recovery points. Unassigned Jobs appear separately. Only Checkpointed indicates a recoverable Git commit.",
  "等待数据":"Waiting for data",
  "正在加载 Task 分类…":"Loading Task categories…",
  "暂无 Task 或 Job":"No Tasks or Jobs yet",
  "首次运行或尚未执行 Job。使用 start_task 建立多阶段任务；没有关联信息的 Job 会自动创建独立 Task。":"Nothing has run yet. Use start_task for multi-stage work; standalone Jobs automatically become separate Tasks.",
  "使用 start_task 创建多阶段 Task；不带上下文的 Job 会自动作为独立任务展示。":"Use start_task for multi-stage work; Jobs without a task context are shown separately.",
  "活动与流量":"Activity & Traffic",
  "最近 60 分钟工具活动、任务并发和公网 HTTP payload 流量。":"Tool activity, concurrent jobs, and public HTTP payload traffic over the last 60 minutes.",
  "最近 60 分钟":"Last 60 minutes",
  "工具请求与完成结果":"Tool requests and outcomes",
  "运行中任务数量":"Number of active jobs",
  "公网 listener RX / TX":"Public listener RX / TX",
  "最近审计事件":"Recent audit events",
  "暂无审计事件":"No audit events","最近任务":"Recent jobs","暂无任务":"No jobs",
  "暂无活动":"No activity","暂无操作日志":"No operations",
  "操作日志":"Operation Log",
  "按时间顺序展示每次操作。主列固定为 Timeline / Operation / Input / Diff，Output 与 Error 仅在展开详情中显示。":"Operations are shown chronologically. The primary columns are Timeline / Operation / Input / Diff; expand an entry to inspect Output and Error.",
  "全部展开":"Expand all","全部折叠":"Collapse all",
  "查看当前 MCP 端点、协议状态、工具 schema revision 和服务端实际暴露的工具分类。":"Inspect the active MCP endpoint, protocol state, schema revision, and the tools exposed by the server.",
  "MCP 工具":"MCP Tools",
  "按职责分类显示当前服务实际暴露的工具；类别可折叠。":"Server tools are grouped by function. Click a group to expand or collapse it.",
  "只展示非敏感运行配置。密钥、Token 和凭据不会通过 Dashboard 返回。":"Only non-sensitive settings are displayed. Keys, tokens, and credentials are not returned by the Dashboard.",

  "当前执行隔离":"Current execution isolation",
  "当前运行":"Active backend",
  "已加载的服务后端":"Backend loaded by the running service",
  "已保存配置":"Saved configuration",
  "配置待重启生效":"Restart required to apply configuration",
  "配置已生效":"Configuration applied",
  "检查配置版本":"Checking configuration revision",
  "资源限制":"Resource limits",
  "隔离能力":"Isolation capabilities",
  "沙箱 / 执行隔离":"Sandbox / execution isolation",
  "打开沙箱管理 ↗":"Open sandbox management ↗",
  "正在检测操作系统":"Detecting operating system",
  "开发中 · 不可选":"In development · Unavailable",
  "检查平台":"Checking platform",
  "appcontainer · Windows 隔离（开发中，暂不可选）":"appcontainer · Windows isolation (in development, unavailable)",
  "执行后端":"Execution backend",
  "选择本机命令执行方式。Windows 和 macOS 不支持 bubblewrap；修改仅写入配置，重启 EndlessVibe 后生效。":"Choose the local command execution backend. Windows and macOS do not support bubblewrap. Saving changes the config only; restart to apply.",
  "执行方式":"Execution mode",
  "disabled · 禁止命令执行":"disabled · No command execution",
  "bubblewrap · Linux 沙箱":"bubblewrap · Linux sandbox",
  "host · 无沙箱（高风险）":"host · No sandbox (high risk)",
  "高风险：host 模式不提供沙箱隔离。启用后，已获执行权限的 Project 和配对父节点可在当前服务账号权限范围内运行本机命令，包括访问该账号可读取的文件与网络。":"High risk: host mode has no sandbox. Authorized Projects and paired parent nodes may run commands with the service account's permissions, including access to its files and network.",
  "我理解无沙箱执行及远程命令的风险，并明确同意启用 host 后端":"I understand the risks of unsandboxed and remote execution and explicitly agree to enable host mode",
  "正在读取当前执行后端…":"Loading execution backend…",
  "保存执行后端":"Save execution backend",
  "必须勾选无沙箱执行风险确认":"Acknowledge the risks of unsandboxed execution before saving",
  "正在验证并保存执行后端…":"Validating and saving execution backend…",
  "已保存执行后端；重启 EndlessVibe 后生效":"Execution backend saved. Restart EndlessVibe to apply.",
  "配置已变化，请核对最新设置后重试":"Configuration changed. Review the latest settings and retry.",
  "执行后端有未保存的更改":"Execution backend has unsaved changes",
  "直接编辑":"Edit directly",
  "。Source 必须是宿主机已存在的绝对路径；Target 必须位于":"The source must be an existing absolute host path. The target must be under",
  "或":"or",
  "。保存只写入配置，不会热重载，重启 EndlessVibe 后生效。":"Saving changes the config only. Restart EndlessVibe to apply them.",
  "只探测基础 bubblewrap namespace 和程序可见性，不挂载 Project、不运行项目代码。":"Checks Bubblewrap namespace creation and program visibility without mounting projects or executing project code.",
  "尚未执行 sandbox diagnostics":"Sandbox diagnostics not run yet",
  "在线修改":"Edit online",
  "；数值通过服务器验证并保存，重启 EndlessVibe 后生效。单位为 bytes / seconds。":"; settings are validated and saved by the server. Restart to apply. Units: bytes / seconds.",
  "修改服务的 Git 可执行文件路径及提交身份。更改不会执行 Git 命令或推送代码，重启后生效。":"Configure the Git executable and commit identity. Saving does not run Git or push code; restart to apply.",
  "开启 Transfer 后发布局域网发现信息，并通过独立 TLS 端口接收配对/授权操作。不会把本地 Dashboard 或 MCP OAuth 入口开放到局域网。":"Enabling Transfer publishes LAN discovery and accepts pairing on a separate TLS port. It does not expose the local Dashboard or MCP OAuth endpoints to the LAN.",
  "Docker Engine Unix socket 具有接近宿主机管理员的权限。默认关闭；启用后必须指定允许访问的精确容器名称，并单独授权启动/停止/重启。普通 development Job 不再默认获得 Docker socket。此处只配置权限，不会立即操作容器。":"The Docker Engine socket grants near-host-admin control. It is disabled by default. Specify exact allowed container names and separate start/stop/restart grants; saving does not operate on containers.",
  "Allowed containers（每行一个精确名称）":"Allowed containers (one exact name per line)",
  "按用途查看状态目录的磁盘占用。缓存扫描最多覆盖 20,000 节点，显示“至少”表示结果不完整。清理执行缓存只影响":"Inspect disk usage by category. Cache scans cover up to 20,000 nodes; “at least” means the result is incomplete. Cleaning execution cache only affects",
  "，后续 Cargo / CMake 构建可能需要重新生成缓存。":"; subsequent Cargo / CMake builds may need to regenerate it.",
  "可清理的构建与工具缓存":"Disposable build and tool cache",
  "持久 Job / Activity / Tasks":"Persistent Jobs / Activity / Tasks",
  "保留，不参与缓存清理":"Retained; excluded from cache cleanup",
  "正在检查任务活动状态…":"Checking active jobs…",
  "Dashboard 127.0.0.1:20001 · uPlot 从 web/vendor 本地加载":"Dashboard 127.0.0.1:20001 · uPlot is served locally from web/vendor",
  "JavaScript 未启用。请访问":"JavaScript is disabled. Visit",
  "查看实时状态。":"for live status.",
  "主导航":"Main navigation","面包屑":"Breadcrumbs",
  "留空时使用目录名":"Leave blank to use the directory name",
  "权限":"Permissions","暂无 Workspace":"No Workspaces",
  "已生效":"Applied","重新加载":"Reload",
  "展开后加载完整 Input / Diff / Output…":"Expand to load full Input / Diff / Output…",
  "加载详情…":"Loading details…","加载任务输出…":"Loading job output…",
  "没有诊断程序":"No diagnostic programs",
  "暂时无法获取状态":"Status temporarily unavailable",
  "事件刷新失败":"Event refresh failed",
  "SSE 重连中":"Reconnecting SSE",
  "配置已变化，已刷新，请重试":"Configuration changed. Refreshed; please retry.",
  "配置已变化，已刷新，请重新填写后保存":"Configuration changed. Refresh the form and retry.",
  "请选择 Workspace":"Select a Workspace",
  "Absolute Path 不能为空":"Absolute Path is required",
  "Project ID 只能包含字母、数字、_、-，最长 64 个字符":"Project ID must contain letters, digits, underscores, or hyphens (max 64).",
  "exec / commit 需要 write":"exec / commit requires write",
  "git rw 需要 write + exec":"git rw requires write + exec",
  "保存中…":"Saving…","正在写入 config.toml…":"Writing config.toml…",
  "权限已写入并已热生效":"Permissions saved and applied without restart",
  "保存失败：":"Save failed: ",
  "未知大小":"Unknown size",
  "Docker socket 必须为绝对路径":"Docker socket must be an absolute path",
  "容器名必须唯一且只含字母、数字、点、下划线、短横线":"Container names must be unique and contain only letters, digits, dots, underscores, or hyphens",
  "启用 Docker MCP 前必须填写容器白名单":"Configure the Docker container allowlist before enabling Docker MCP",
  "请确认 Docker socket 可控制宿主机的风险":"Acknowledge that the Docker socket can control the host",
  "正在验证并保存 Docker 安全配置…":"Validating and saving Docker security settings…",
  "Docker 配置已保存，需要重启 EndlessVibe 才能使用新权限":"Docker settings saved. Restart EndlessVibe to apply them.",
  "配置已发生变更，请核对新值后重新编辑":"Configuration changed. Review the latest values before editing.",
  "Git executable 必须为绝对路径":"Git executable must be an absolute path",
  "正在写入 [git]…":"Writing [git]…",
  "Git 配置已保存；重启 EndlessVibe 后生效":"Git settings saved; restart EndlessVibe to apply them.",
  "配置已由其他操作修改，已加载最新值；请重新编辑":"Configuration was modified elsewhere. The latest values have been loaded.",
  "Max read 不得大于 Max file":"Max read cannot exceed Max file",
  "正在验证并写入 [limits]…":"Validating and saving [limits]…",
  "Limits 已保存；重启 EndlessVibe 后生效":"Limits saved; restart EndlessVibe to apply them.",
  "需要已确认的 host backend；该 Project 可运行 PATH 中任意程序":"Requires an acknowledged host backend; this Project may run any executable on PATH.",
  "没有运行中的 Job。可以确认清理执行缓存，下一次构建会重新生成；Git / Backups / SQLite 不受影响。":"No jobs are running. You can clear the execution cache; later builds will regenerate it. Git / Backups / SQLite are not affected.",
  "正在创建基础 bubblewrap namespace 并检查工具可见性…":"Creating the Bubblewrap namespace and checking tool availability…",
  "Sandbox required programs 可见":"Sandbox required programs are available",
  "清理执行缓存中，请勿关闭页面或启动新 Job…":"Clearing execution cache; do not close the page or start new jobs…",
  "缓存当前被 Job 使用，待所有 Job 完成后重试":"Cache is currently in use. Retry after running jobs complete.",
  "正在清理过期记录并压缩 SQLite…":"Removing expired records and compacting SQLite…",
  "有运行中的 Job，数据库压缩需在空闲时执行":"Database compaction requires all jobs to be idle",
  "正在执行受限 retention cleanup…":"Running bounded retention cleanup…",
  "正在重新验证并加载 Workspace / Project 授权…":"Revalidating and loading Workspace / Project authorizations…",
  "Docker 安全配置有未保存的修改":"Unsaved Docker security settings",
  "Git 配置有未保存的更改":"Unsaved Git settings",
  "Limits 有未保存的更改":"Unsaved Limits settings",
  "有未保存的 readonly mount 修改":"Unsaved read-only mount changes",
  "当前没有 readonly mount。点击 Add mount 添加。":"No read-only mounts. Click Add mount to create one.",
  "填写 Source / Target 后保存；保存后需要重启 EndlessVibe":"Enter Source and Target, then save. Restart EndlessVibe to apply.",
  "每一行都必须同时填写 Source 和 Target":"Each row requires both Source and Target",
  "正在写入 [execution].readonly_mounts…":"Writing [execution].readonly_mounts…",
  "配置在编辑期间已变化，已重新加载当前 readonly mounts，请重新修改后保存":"Configuration changed during editing. Current read-only mounts were reloaded; please retry.",
  "没有可授权的本地 Project":"No local Projects available for authorization",
  "尚无本地 Project。可先完成节点配对，之后到 Projects 页面添加并单独授权。":"No local Projects yet. Pair the nodes now, then register and explicitly grant Projects from the Projects page.",
  "可选：配对时授予 Project":"Optional: grant Projects during pairing",
  "配对后前往 Projects 添加 Project":"Register a Project after pairing",
  "两端必须核对相同的六位验证码。配对不自动授予 Project 权限。":"Verify the same six-digit code on both devices. Pairing does not automatically grant Project access.",
  "管理 Project 授权":"Manage Project grants",
  "保存 Project 授权":"Save Project grants",
  "已配对 · 尚未授权 Project（可随时添加，无需重新配对）":"Paired · no authorized Projects yet (add them later without pairing again)",
  "在 Projects 中添加新项目后，回到这里勾选并保存。取消勾选即撤销授权；不会自动授权。":"After registering Projects on the Projects page, return here to select and save their grants. Deselect to revoke; never auto-grant.",
  "前往 Projects 添加 Project":"Open Projects to add a Project",
  "已勾选的 Project 必须至少授予一种权限；如需撤销，请取消勾选该 Project":"Each selected Project needs at least one permission; deselect it to revoke access",
  "已批准节点":"Approved node",
  "尚未授权 Project":"No authorized Projects yet",
  "当前授权：":"Current grants: ",
  "没有可显示的新请求记录。旧请求仍可通过 Request status 按 ID 查询。":"No indexed requests found. Older requests can be retrieved by ID using Request status.",
  "请输入此前远程操作的 request_id":"Enter the request_id from the previous remote operation",
  "请填写相对文件路径":"Enter a relative file path",
  "请先点击文件列表中的文件，或输入相对文件路径（例如 README.md）":"Select a file from the list or enter a relative path (e.g. README.md)",
  "点击文件列表中的文件即可读取，或输入项目内的相对路径。":"Click a file to read it, or enter a project-relative path.",
  "↑ 上一级":"↑ Parent folder","目录为空":"Empty directory","加载更多文件":"Load more files",
  "选择文件或 Git 操作。子节点会再次验证该 Project 的读取授权。":"Choose a file or Git operation. The child rechecks this Project's read grant.",
  "选择左侧 Project 查看详情。":"Select a Project on the left to see details.",
  "没有授权可见的 Project":"No authorized Projects visible",
  "尚无已配对节点":"No paired nodes",
  "请先选择配对节点的 Project":"Select a Project on a paired node first",
  "Arguments 必须是 JSON object":"Arguments must be a JSON object",
  "不能更改当前选中的 Workspace/Project":"Cannot change the selected Workspace / Project",
  "已发送配对申请。两端验证码：":"Pairing requested. Verification code on both devices: ",
  "Transfer 指标暂不可用；不影响节点配对及项目访问":"Transfer metrics are unavailable; pairing and Project access still work.",
  "Transfer disabled · 在 Config 中启用并重启":"Transfer disabled · enable it in Settings and restart",
  "Transfer 配置已保存。重启 EndlessVibe 后生效。":"Transfer settings saved. Restart EndlessVibe to apply them.",
  "Transfer 设置有未保存的修改":"Unsaved Transfer settings",
  "其他":"Other",
  "暂无工具":"No tools available"
}));

/* English labels that are already in the HTML and need Chinese equivalents. */
const enToZh = new Map(Object.entries({
  "GET STARTED":"开始使用","CONNECT TO CHATGPT":"连接 CHATGPT","CONNECTION SETUP":"连接配置",
  "Home":"首页","Projects":"项目","Tasks":"任务","Nodes":"节点","Activity":"活动",
  "Operations":"操作记录","Operation Log":"操作日志","Settings":"配置",
  "Dashboard":"仪表盘","Refresh nodes":"刷新节点","Pair Child":"配对子节点",
  "Request pairing":"申请配对","Discovered Nodes":"已发现节点",
  "Pending Pairing":"待确认配对","Trusted Nodes / Remote Projects":"已信任节点 / 远程项目",
  "Recorded requests":"已记录请求","Needs review":"待核查请求",
  "Completed requests":"已完成请求","Call success · 24h":"24 小时成功率",
  "Recent remote requests":"最近远程请求","Request history":"请求历史",
  "Request status":"请求状态","Load older requests":"加载更早请求",
  "Inspect":"查看","Remote Project":"远程项目","Remote path":"远程路径",
  "Remote operations · advanced":"远程高级操作","Review and submit to child":"审核并提交至子节点",
  "Run command":"运行命令","Get job":"查询任务","Get job output":"查询任务输出",
  "Cancel job":"取消任务","Create directory":"创建目录",
  "Write file":"写入文件","Apply patch":"应用补丁","Git commit":"Git 提交",
  "Operation":"操作","Arguments JSON":"参数 JSON","Project ID":"项目 ID",
  "Execution Profile":"执行配置","Absolute Path":"绝对路径",
  "Workspace / Project":"工作空间 / 项目","Add Project":"添加项目",
  "Save Git":"保存 Git 配置","Save limits":"保存限制","Save Transfer settings":"保存 Transfer 配置",
  "Save Docker settings":"保存 Docker 配置",
  "Enable Transfer":"启用 Transfer","Advertise LAN node":"广播局域网节点",
  "Discover LAN nodes":"发现局域网节点",
  "Project":"项目","Workspace":"工作空间","Active Jobs":"活跃任务",
  "Execution":"执行环境","Tool Requests":"工具请求","Success":"成功",
  "Failed":"失败","HTTP Requests":"HTTP 请求","Tool Latency":"工具耗时",
  "Queue Wait":"排队等待","Job Stops":"任务结束","Git Commit":"Git 提交",
  "Git Push":"Git 推送","Tool Activity":"工具活动","Network Traffic":"网络流量",
  "Timeline":"时间线","Input":"输入","Diff":"差异",
  "Endpoint":"端点","Protocol":"协议","Tools":"工具",
  "Readonly Mounts":"只读挂载","Check Sandbox":"检查沙箱",
  "Storage Health":"存储状态","Execution Cache":"执行缓存",
  "Clear execution cache":"清理执行缓存","Compact SQLite":"压缩 SQLite",
  "Prune records":"清理过期记录","Backups":"备份",
  "Save readonly mounts":"保存只读挂载",
  "Docker MCP":"Docker MCP","Transfer / LAN Nodes":"Transfer / 局域网节点",
  "Reload project config":"重新加载项目配置",
  "Save":"保存","Cancel":"取消","Expand all":"全部展开",
  "Collapse all":"全部折叠","Requests":"请求数",
  "Active":"活跃","Auto Jobs":"自动任务","Checkpoints":"检查点",
  "Attention":"待处理","Running":"运行中","Pending":"等待中",
  "Completed":"已完成","Failed":"失败","Enabled":"已启用","Disabled":"已禁用",
  "Connection":"连接","Files":"文件","Commands":"命令","Other":"其他",
  "Theme":"主题","Language":"语言","Dark":"深色","Light":"亮色",
  "SERVICE OVERVIEW":"服务概览","LAN NODE MESH":"局域网节点网络",
  "PROJECT MANAGEMENT":"项目管理","TASK PROGRESS":"任务进度",
  "OPERATION TIMELINE":"操作时间线","MODEL CONTEXT PROTOCOL":"模型上下文协议",
  "CONFIGURATION":"配置管理","RESTART REQUIRED":"需要重启",
  "WRITE CONFIG":"写入配置","ACCESS / GIT":"授权 / Git",
  "NODE ID":"节点 ID","SITE":"站点","STATUS":"状态",
  "STORAGE":"存储","AUDIT":"审计","JOBS":"任务",
  "TRANSFER":"传输","Request":"请求",
  "No activity":"暂无活动","No jobs":"暂无任务",
  "No operations":"暂无操作","No tools available":"暂无工具",
  "Loading…":"加载中…"
}));

const partialEn = [
  ["保存失败：","Save failed: "],["已保存 ","Saved "],["保存中","Saving"],
  ["已批准节点 ","Approved node "],["没有授权可见的 Project","No authorized Projects visible"],
  [" 的本地配对？两端均须撤销才能完全解除双向信任。"," from this device? Both nodes must revoke pairing to fully remove trust."],
  ["撤销节点 ","Revoke node "],["本地已撤销 ","Revoked locally "],
  ["请在两台设备本地核对验证码 ","Compare the verification code on both devices: "],
  [" 一致，并确认授权 "," matches, then confirm authorization for "],
  ["；对方也必须在本地批准","; the other device must approve locally too"],
  ["；请在子节点 Nodes 页面核对。","; verify it on the child's Nodes page."],
  ["正在更新 ","Updating "],["更新于 ","Updated "],["更新于","Updated"],
  ["最近成功检查：","Last successful check: "],
  ["缺失 required programs:","Missing required programs:"],
  ["有未保存的","Unsaved "],["配置已变化","Configuration changed"],
  ["正在加载","Loading "],["已完成：删除 ","Completed: deleted "],
  [" 条记录；未执行 VACUUM"," records; VACUUM was not performed"],
  ["正在执行受限 ","Running bounded "],
  ["服务端工具 schema revision：","Server tool schema revision: "],
  ["如果这里的 revision / tool count 已更新，但 ChatGPT 中仍缺少新工具，说明客户端仍缓存旧 schema，需要重新连接或刷新 EndlessVibe 插件。","If revision or tool count has changed but ChatGPT still lacks tools, reconnect or refresh the EndlessVibe plugin."],
  ["已保存","Saved"],["需要重启 EndlessVibe","Restart EndlessVibe to apply"],
  ["配置","Settings"],["已热生效","applied without restart"],
  ["重启后生效","applies after restart"],["正在验证","Validating"],
  ["正在写入","Writing"],["当前有 ","There are "],
  [" 个 Job（或缓存正在使用），请在 Job 完成后再清理。"," active Jobs (or cache is in use). Retry after they finish."],
  [" 个独立 Job"," standalone Jobs"],[" 个多阶段任务"," multi-stage Tasks"],
  [" 个阶段和 "," stages and "],["正在运行 Job 时服务器会拒绝清理。","The server rejects cleanup while Jobs are running."],
  ["目录：","Directory: "],[" 项"," items"],
  ["远程操作未确认：","Remote operation unconfirmed: "],
  ["请使用 Request status 查询，禁止盲目重放。","Query Request status; do not replay blindly."],
  ["远程操作 ","Remote operation "],[" 已返回结果；"," returned a result; "],
  ["确认向子节点执行 ","Confirm operation on child node: "],
  ["远程修改可能继续在子节点运行，即使当前网络连接断开。","Remote mutations may continue running even after this network connection closes."],
  ["禁止盲目重放","Do not replay blindly"],
  ["近 24h ","Past 24h: "],[" 次远程调用"," remote calls"],
  [" 失败"," failed"],[" 中断"," interrupted"],
  [" 条过期响应已留存去重标记"," expired responses retain deduplication records"],
  [" 操作"," operations"],["状态：","Status: "],
  ["运行中 · SSE","Running · SSE"]
];

/* Untranslated technical output and user-owned values must never be rewritten. */
const excluded = "script,style,pre,code,textarea,select,[contenteditable],[data-i18n-ignore],.node-output,.operation-cell-preview,.operation-json,.operation-diff,.operation-job-output,.mono-value,.activity-target,.operation-target,.tool-tree-item,.task-identity,.node-history-meta strong";
const textCache = new WeakMap();
const attrCache = new WeakMap();
const attributes = ["placeholder", "title", "aria-label"];
let language = "zh-CN";
let theme = "dark";
let observer = null;

function preserveSpace(source, translated) {
  const trimmed = source.trim();
  if (!trimmed) return source;
  const left = source.slice(0, source.indexOf(trimmed));
  const right = source.slice(source.indexOf(trimmed) + trimmed.length);
  return left + translated + right;
}

/** Pure translation helper; direct lookup takes priority over fragment matches. */
export function translate(source, target = language) {
  if (source === undefined || source === null) return source;
  const text = String(source);
  const stripped = text.trim();
  if (!stripped) return text;
  if (target === "zh-CN") {
    const value = enToZh.get(stripped);
    return value === undefined ? text : preserveSpace(text, value);
  }
  const exact = zhToEn.get(stripped);
  if (exact !== undefined) return preserveSpace(text, exact);
  if (!/[\u3400-\u9fff]/u.test(text)) return text;
  let result = text;
  for (const [zh, en] of partialEn) {
    if (result.includes(zh)) result = result.split(zh).join(en);
  }
  return result;
}

export const getLanguage = () => language;
export const getTheme = () => theme;

function excludedNode(element) {
  return element?.closest?.(excluded) !== null;
}

function updateText(node) {
  if (excludedNode(node.parentElement)) return;
  const current = node.nodeValue;
  const state = textCache.get(node);
  const source = state && current === state.rendered ? state.source : current;
  const rendered = translate(source);
  textCache.set(node, { source, rendered });
  if (current !== rendered) node.nodeValue = rendered;
}

function updateAttributes(element) {
  if (excludedNode(element)) return;
  let states = attrCache.get(element);
  if (!states) { states = new Map(); attrCache.set(element, states); }
  for (const attr of attributes) {
    if (!element.hasAttribute(attr)) continue;
    const current = element.getAttribute(attr);
    const previous = states.get(attr);
    const source = previous && current === previous.rendered ? previous.source : current;
    const rendered = translate(source);
    states.set(attr, { source, rendered });
    if (current !== rendered) element.setAttribute(attr, rendered);
  }
}

function translateTree(root) {
  if (root.nodeType === Node.TEXT_NODE) { updateText(root); return; }
  if (root.nodeType !== Node.ELEMENT_NODE) return;
  if (excludedNode(root)) return;
  updateAttributes(root);
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT);
  while (walker.nextNode()) {
    const node = walker.currentNode;
    if (node.nodeType === Node.TEXT_NODE) updateText(node);
    else updateAttributes(node);
  }
}

function updatePreference(key, value, persist = true) {
  if (key === "theme") theme = value === "light" ? "light" : "dark";
  if (key === "language") language = value === "en" ? "en" : "zh-CN";
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.dataset.language = language;
  root.lang = language;
  root.style.colorScheme = theme;
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.content = theme === "light" ? "#f4f7fc" : "#0b1018";
  const themeSelect = document.getElementById("theme-select");
  const languageSelect = document.getElementById("language-select");
  if (themeSelect) themeSelect.value = theme;
  if (languageSelect) languageSelect.value = language;
  if (persist) {
    try { localStorage.setItem("endlessvibe." + key, key === "theme" ? theme : language); } catch (_) {}
  }
  translateTree(document.body);
  document.dispatchEvent(new CustomEvent("endlessvibe:preferences", {
    detail: { language, theme }
  }));
}

export function setLanguage(value) { updatePreference("language", value); }
export function setTheme(value) { updatePreference("theme", value); }

export function initPreferences() {
  language = document.documentElement.dataset.language === "en" ? "en" : "zh-CN";
  theme = document.documentElement.dataset.theme === "light" ? "light" : "dark";
  document.getElementById("language-select")?.addEventListener("change", e => setLanguage(e.target.value));
  document.getElementById("theme-select")?.addEventListener("change", e => setTheme(e.target.value));
  updatePreference("language", language, false);
  if (!observer) {
    observer = new MutationObserver(records => {
      for (const record of records) {
        if (record.type === "characterData") updateText(record.target);
        else if (record.type === "attributes") updateAttributes(record.target);
        else for (const child of record.addedNodes) translateTree(child);
      }
    });
    observer.observe(document.body, {
      subtree: true, childList: true, characterData: true,
      attributes: true, attributeFilter: attributes
    });
  }
  window.addEventListener("storage", e => {
    if (e.key === "endlessvibe.theme") updatePreference("theme", e.newValue, false);
    if (e.key === "endlessvibe.language") updatePreference("language", e.newValue, false);
  });
}
