"use strict";
const $=id=>document.getElementById(id);
const currentPage=location.pathname==="/activity"?"activity":location.pathname==="/config"?"config":"home";
const pageNames={home:"首页",activity:"活动",config:"配置"};
const plots=new Map();
let busy=false;
const dateTime=new Intl.DateTimeFormat("zh-CN",{year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false});
const timeOnly=new Intl.DateTimeFormat("zh-CN",{hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false});
const setText=(id,value)=>{const el=$(id);if(el)el.textContent=value??"—";};
const formatTime=seconds=>dateTime.format(new Date(seconds*1000));
const formatShortTime=seconds=>timeOnly.format(new Date(seconds*1000));
function formatUptime(seconds){const days=Math.floor(seconds/86400),hours=Math.floor(seconds%86400/3600),minutes=Math.floor(seconds%3600/60),secs=seconds%60;return (days?`${days}天 `:"")+`${String(hours).padStart(2,"0")}:${String(minutes).padStart(2,"0")}:${String(secs).padStart(2,"0")}`;}
function formatBytes(value){let n=Number(value)||0;const units=["B","KiB","MiB","GiB","TiB"];let i=0;while(n>=1024&&i<units.length-1){n/=1024;i++;}return `${n>=100||i===0?n.toFixed(0):n>=10?n.toFixed(1):n.toFixed(2)} ${units[i]}`;}
function node(tag,className,text){const el=document.createElement(tag);if(className)el.className=className;if(text!==undefined)el.textContent=text;return el;}
function activatePage(){
    document.querySelectorAll("[data-page]").forEach(el=>el.hidden=el.dataset.page!==currentPage);
    document.querySelectorAll("[data-nav]").forEach(el=>el.classList.toggle("active",el.dataset.nav===currentPage));
    setText("breadcrumb-current",pageNames[currentPage]);
    document.title=`EndlessVibe · ${pageNames[currentPage]}`;
}
function setConnection(online){
    const badge=$("header-status");if(!badge)return;
    badge.className=`header-status ${online?"online":"offline"}`;setText("header-status-text",online?"运行中":"离线");
}
function renderToolChips(id,tools){const host=$(id);if(!host)return;host.replaceChildren();for(const tool of tools||[])host.appendChild(node("span","tool-chip",tool));if(!(tools||[]).length)host.appendChild(node("span","empty-inline","暂无工具"));}
function chartTheme(){const s=getComputedStyle(document.documentElement);return{muted:s.getPropertyValue("--muted").trim(),border:s.getPropertyValue("--border").trim(),accent:s.getPropertyValue("--accent").trim(),red:s.getPropertyValue("--red").trim(),blue:s.getPropertyValue("--blue").trim(),amber:s.getPropertyValue("--amber").trim(),violet:s.getPropertyValue("--violet").trim()};}
function chartOptions(host,series,height=270,yFormatter=null){
    const c=chartTheme(),xAxis={stroke:c.muted,grid:{stroke:c.border,width:1},ticks:{stroke:c.border,width:1},font:"11px system-ui"},yAxis={stroke:c.muted,grid:{stroke:c.border,width:1},ticks:{stroke:c.border,width:1},font:"11px system-ui"};
    if(yFormatter)yAxis.values=(_,values)=>values.map(yFormatter);
    return{width:Math.max(280,Math.floor(host.clientWidth)),height,scales:{x:{time:true},y:{auto:true}},axes:[xAxis,yAxis],series,legend:{show:true},cursor:{drag:{x:true,y:false,setScale:true}}};
}
function upsertPlot(key,hostId,data,series,height=270,yFormatter=null){
    const host=$(hostId);if(!host||host.hidden||host.clientWidth===0)return;
    const existing=plots.get(key);
    if(existing){existing.setData(data);return;}
    plots.set(key,new uPlot(chartOptions(host,series,height,yFormatter),data,host));
}
function renderMetrics(data){
    const totals=data.totals||{},points=data.points||[];
    for(const prefix of ["home","activity"]){
        setText(`${prefix}-requests`,totals.requests??0);setText(`${prefix}-success`,totals.successes??0);setText(`${prefix}-failed`,totals.failures??0);setText(`${prefix}-http-requests`,totals.http_requests??0);setText(`${prefix}-rx`,formatBytes(totals.rx_bytes));setText(`${prefix}-tx`,formatBytes(totals.tx_bytes));
    }
    if(!points.length||!window.uPlot)return;
    const times=points.map(p=>p.time),requests=points.map(p=>p.requests),successes=points.map(p=>p.successes),failures=points.map(p=>p.failures),active=points.map(p=>p.active_jobs),rx=points.map(p=>p.rx_bytes),tx=points.map(p=>p.tx_bytes),c=chartTheme();
    const toolSeries=[{}, {label:"Requests",stroke:c.blue,width:2,points:{show:false}}, {label:"Success",stroke:c.accent,width:2,points:{show:false}}, {label:"Failed",stroke:c.red,width:2,points:{show:false}}];
    const trafficSeries=[{}, {label:"RX",stroke:c.violet,width:2,points:{show:false},value:(_,v)=>v==null?"—":formatBytes(v)}, {label:"TX",stroke:c.amber,width:2,points:{show:false},value:(_,v)=>v==null?"—":formatBytes(v)}];
    const jobsSeries=[{}, {label:"Active Jobs",stroke:c.amber,width:2,points:{show:false}}];
    if(currentPage==="home"){upsertPlot("home-tool","home-tool-chart",[times,requests,successes,failures],toolSeries,245);upsertPlot("home-traffic","home-traffic-chart",[times,rx,tx],trafficSeries,245,formatBytes);}
    if(currentPage==="activity"){upsertPlot("activity-tool","activity-tool-chart",[times,requests,successes,failures],toolSeries,285);upsertPlot("activity-jobs","activity-jobs-chart",[times,active],jobsSeries,285);upsertPlot("activity-traffic","activity-traffic-chart",[times,rx,tx],trafficSeries,285,formatBytes);setText("activity-updated",`更新于 ${formatTime(data.generated_at)} · ${data.bucket_seconds}s/点`);}
}
function outcomeClass(outcome){return ["succeeded","running","accepted"].includes(outcome)?"good":["failed","timed_out","cancelled","interrupted"].includes(outcome)?"bad":"neutral";}
function renderAudits(id,audits,limit){const host=$(id);if(!host)return;host.replaceChildren();const rows=(audits||[]).slice(0,limit);if(!rows.length){host.className="activity-list empty-state";host.textContent="暂无活动";return;}host.className="activity-list";for(const item of rows){const row=node("div","activity-row");const main=node("div","activity-main");main.append(node("strong","activity-tool",item.tool||"—"),node("span","activity-target",item.workspace||"—"));const meta=node("div","activity-meta");meta.append(node("span",`state ${outcomeClass(item.outcome)}`,item.outcome||"—"),node("time","",formatShortTime(item.time)));row.append(main,meta);host.appendChild(row);}}
function renderJobs(id,jobs,limit){const host=$(id);if(!host)return;host.replaceChildren();const rows=(jobs||[]).slice(0,limit);if(!rows.length){host.className="activity-list empty-state";host.textContent="暂无任务";return;}host.className="activity-list";for(const item of rows){const row=node("div","activity-row");const main=node("div","activity-main");const target=item.project?`${item.workspace}/${item.project}`:item.workspace||"—";main.append(node("strong","activity-tool",item.program||"—"),node("span","activity-target",target));const meta=node("div","activity-meta");meta.append(node("span",`state ${outcomeClass(item.status)}`,item.status||"—"),node("time","",formatShortTime(item.created)));row.append(main,meta);host.appendChild(row);}}
function displayValue(value){if(Array.isArray(value))return value.length?value.join(", "):"—";if(typeof value==="boolean")return value?"启用":"禁用";if(value===null||value===undefined||value==="")return"—";return String(value);}
function renderConfigList(id,rows){const host=$(id);if(!host)return;host.replaceChildren();for(const [label,value] of rows){const wrap=node("div","config-row");wrap.append(node("dt","",label),node("dd","mono-value",displayValue(value)));host.appendChild(wrap);}}
function renderWorkspaceConfig(workspaces){const host=$("workspace-config");if(!host)return;host.replaceChildren();for(const workspace of workspaces||[]){const card=node("article","workspace-card");const head=node("div","workspace-head");const title=node("div");title.append(node("strong","",workspace.id),node("span","mono-value",workspace.path));head.append(title,node("span","count-badge",`${workspace.project_count} projects`));card.appendChild(head);const projects=node("div","project-list");for(const project of workspace.projects||[]){const row=node("div","project-row");const left=node("div");left.append(node("strong","",project.id),node("span","mono-value",project.path));const perms=node("div","permission-list");for(const [label,ok] of [["write",project.allow_write],["exec",project.allow_exec],["commit",project.allow_git_commit]])perms.appendChild(node("span",`permission ${ok?"on":"off"}`,label));row.append(left,perms);projects.appendChild(row);}card.appendChild(projects);host.appendChild(card);}if(!(workspaces||[]).length)host.textContent="暂无 Workspace";}
function renderConfig(data){
    renderConfigList("config-server",[["Bind",data.server?.bind],["Public URL",data.server?.public_url],["Allowed Hosts",data.server?.allowed_hosts],["Dashboard",data.dashboard?.listen_address],["Local Only",data.dashboard?.local_only]]);
    renderConfigList("config-security",[["Authentication",data.security?.authentication],["HTTP Loopback",data.security?.allow_http_loopback],["Access Token",`${data.security?.access_token_seconds??0}s`],["Refresh Token",`${data.security?.refresh_token_seconds??0}s`],["Extra Redirect URIs",data.security?.extra_redirect_uri_count]]);
    renderConfigList("config-execution",[["Backend",data.execution?.backend],["Shell",data.execution?.allow_shell],["Network",data.execution?.allow_network],["Bubblewrap",data.execution?.bubblewrap],["PATH",data.execution?.path],["Memory",`${data.execution?.memory_limit_mb??0} MiB`],["Max Processes",data.execution?.max_processes],["Allowed Programs",data.execution?.allowed_programs],["Readonly Mounts",(data.execution?.readonly_mounts||[]).map(m=>`${m.source} → ${m.target}`)]]);
    renderConfigList("config-limits",[["Max File",formatBytes(data.limits?.max_file_bytes)],["Max Read",formatBytes(data.limits?.max_read_bytes)],["Max Output",formatBytes(data.limits?.max_output_bytes)],["Command Timeout",`${data.limits?.command_timeout_seconds??0}s`],["Max Jobs",data.limits?.max_jobs],["Retained Jobs",data.limits?.retained_jobs],["Search Max Files",data.limits?.search_max_files],["Search Max Bytes",formatBytes(data.limits?.search_max_bytes)]]);
    renderConfigList("config-git",[["Executable",data.git?.executable],["Author",data.git?.author_name],["Email",data.git?.author_email]]);
    renderWorkspaceConfig(data.workspaces);setText("mcp-tool-count",`${data.mcp?.tool_count??0} tools`);renderToolChips("mcp-tools",data.mcp?.tools||[]);
}
function renderStatus(data){
    setConnection(true);setText("version",`v${data.version}`);setText("uptime",formatUptime(data.uptime_seconds));setText("workspace-count",data.workspace_count);setText("project-count",data.project_count);setText("active-jobs",data.active_jobs);setText("execution-backend",data.security?.execution_backend);setText("status-title","服务运行正常");setText("status-detail","MCP 与本地 Dashboard 均已响应；首页数据每 5 秒更新。");setText("last-success",`最近成功检查：${formatTime(data.checked_at_unix_seconds)}`);
    setText("home-auth",data.security?.authentication);setText("home-shell",data.security?.shell_enabled?"启用":"禁用");setText("home-network",data.security?.network_enabled?"启用":"禁用");setText("home-tool-count",Array.isArray(data.mcp?.tools)?data.mcp.tools.length:"—");renderToolChips("home-mcp-tools",data.mcp?.tools||[]);
}
async function getJson(url,signal){const response=await fetch(url,{cache:"no-store",headers:{Accept:"application/json"},signal});if(!response.ok)throw new Error(`${url} HTTP ${response.status}`);return response.json();}
async function refresh(){
    if(busy)return;busy=true;const button=$("refresh");if(button){button.disabled=true;button.textContent="刷新中…";}
    const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),4500);
    try{
        const status=await getJson("/api/status",controller.signal);renderStatus(status);
        if(currentPage==="home"||currentPage==="activity"){
            const [metrics,activity]=await Promise.all([getJson("/api/metrics",controller.signal),getJson("/api/activity",controller.signal)]);renderMetrics(metrics);renderAudits("home-activity-list",activity.audits,8);setText("home-activity-updated",`更新于 ${formatShortTime(activity.generated_at)}`);if(currentPage==="activity"){renderAudits("audit-list",activity.audits,40);renderJobs("job-list",activity.jobs,30);}
        }
        if(currentPage==="config")renderConfig(await getJson("/api/config",controller.signal));
    }catch(error){setConnection(false);setText("status-title","暂时无法获取状态");setText("status-detail",error.name==="AbortError"?"请求超时，请检查 EndlessVibe 服务。":error.message);}
    finally{clearTimeout(timeout);busy=false;if(button){button.disabled=false;button.textContent="立即刷新";}}
}
function resizePlots(){for(const [key,plot] of plots){const host=plot.root?.parentElement;if(host&&host.clientWidth>0)plot.setSize({width:Math.max(280,Math.floor(host.clientWidth)),height:key.startsWith("home-")?245:285});}}
activatePage();setText("site-origin",location.origin);
$("refresh")?.addEventListener("click",refresh);
document.addEventListener("visibilitychange",()=>{if(!document.hidden)refresh();});
window.addEventListener("online",refresh);
if("ResizeObserver"in window)new ResizeObserver(resizePlots).observe(document.querySelector(".shell"));else window.addEventListener("resize",resizePlots);
setInterval(()=>{if(!document.hidden&&currentPage!=="config")refresh();},5000);
refresh();
