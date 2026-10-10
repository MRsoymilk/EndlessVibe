/*
 * EndlessVibe UI localization.
 * The Chinese copy already exists in the app. We translate presentation text only,
 * never code blocks, tool output, project paths, secrets, or user input values.
 */
const zhToEn = new Map(Object.entries({
  "首页":"Home","活动":"Activity","日志":"Logs","配置":"Settings","任务":"Tasks","项目":"Projects",
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
  "局域网发现、加密配对与远程 Project 管理。发现不等于授权；必须在两台设备上核对验证码并分别批准。":"Discover, securely pair, and manage remote projects over the LAN. Discovery is not authorization: verify the matching code on both devices and approve both sides.",
  "等待扫描":"Waiting for scan",
  "从下方发现列表选择节点，或手动输入子节点的局域网 IP:Transfer 端口。配对仅创建等待确认的申请，不直接授权 Project。":"Choose a discovered node below or enter the child node's LAN IP and Transfer port. Pairing only creates a pending request; it does not grant project access.",
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
  "写入 config.toml；目标目录必须已存在，并且严格位于所选 Workspace 根目录下。保存后会立即尝试热重载到 MCP 运行时；若同时存在服务级配置变化，则保留配置并提示需要重启。":"Writes to config.toml. The target directory must already exist inside the selected Workspace root. Changes attempt a hot reload; service-level changes may require a restart.",
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
  "Absolute Path 必须是绝对路径":"Absolute Path must be absolute",
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
  "没有可显示的新请求记录。旧请求仍可通过 Request status 按 ID 查询。":"No indexed requests found. Older requests can be retrieved by ID using Request status.",
  "请输入此前远程操作的 request_id":"Enter the request_id from the previous remote operation",
  "请填写相对文件路径":"Enter a relative file path",
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
