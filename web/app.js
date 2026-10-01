"use strict";
const $=id=>document.getElementById(id);
const refreshButton=$("refresh");
let busy=false,activityPlot=null,jobsPlot=null;
const dateTime=new Intl.DateTimeFormat("zh-CN",{year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false});
const formatTime=seconds=>dateTime.format(new Date(seconds*1000));
function formatUptime(seconds){const days=Math.floor(seconds/86400),hours=Math.floor(seconds%86400/3600),minutes=Math.floor(seconds%3600/60),secs=seconds%60;return (days?`${days}天 `:"")+`${String(hours).padStart(2,"0")}:${String(minutes).padStart(2,"0")}:${String(secs).padStart(2,"0")}`;}
function setConnection(online,detail){$("status-badge").className=`badge ${online?"online":"offline"}`;$("badge-text").textContent=online?"运行中":"无法连接";$("status-title").textContent=online?"服务运行正常":"暂时无法获取状态";$("status-detail").textContent=detail;document.querySelector(".metrics").classList.toggle("stale",!online);}
function chartTheme(){const s=getComputedStyle(document.documentElement);return{muted:s.getPropertyValue("--muted").trim(),border:s.getPropertyValue("--border").trim(),accent:s.getPropertyValue("--accent").trim(),red:s.getPropertyValue("--red").trim(),blue:s.getPropertyValue("--blue").trim(),amber:s.getPropertyValue("--amber").trim()};}
function chartOptions(host,series){const c=chartTheme(),axis={stroke:c.muted,grid:{stroke:c.border,width:1},ticks:{stroke:c.border,width:1},font:"11px system-ui"};return{width:Math.max(280,Math.floor(host.clientWidth)),height:270,scales:{x:{time:true},y:{auto:true}},axes:[axis,axis],series,legend:{show:true},cursor:{drag:{x:true,y:false,setScale:true}}};}
function renderMetrics(data){
    if(!window.uPlot)throw new Error("本地 uPlot 未加载");
    if(!Array.isArray(data.points)||!data.points.length)throw new Error("指标数据为空");
    const times=data.points.map(p=>p.time),requests=data.points.map(p=>p.requests),successes=data.points.map(p=>p.successes),failures=data.points.map(p=>p.failures),active=data.points.map(p=>p.active_jobs),c=chartTheme();
    const activityData=[times,requests,successes,failures],jobsData=[times,active];
    const activityHost=$("activity-chart"),jobsHost=$("jobs-chart");
    if(activityPlot)activityPlot.setData(activityData);else activityPlot=new uPlot(chartOptions(activityHost,[{}, {label:"Requests",stroke:c.blue,width:2}, {label:"Success",stroke:c.accent,width:2}, {label:"Failed",stroke:c.red,width:2}]),activityData,activityHost);
    if(jobsPlot)jobsPlot.setData(jobsData);else jobsPlot=new uPlot(chartOptions(jobsHost,[{}, {label:"Active Jobs",stroke:c.amber,width:2}]),jobsData,jobsHost);
    $("requests-1h").textContent=data.totals?.requests??"—";$("success-1h").textContent=data.totals?.successes??"—";$("failed-1h").textContent=data.totals?.failures??"—";$("active-jobs-chart").textContent=active.at(-1)??"—";
    $("metrics-status").textContent=`更新于 ${formatTime(data.generated_at)} · ${data.bucket_seconds}s/点`;
}
async function refreshMetrics(){
    try{
        const response=await fetch("/api/metrics",{cache:"no-store",headers:{Accept:"application/json"}});
        if(!response.ok)throw new Error(`HTTP ${response.status}`);
        renderMetrics(await response.json());
    }catch(error){
        $("metrics-status").textContent=`曲线不可用：${error.message}`;
    }
}
async function refresh(){
    if(busy)return;
    busy=true;refreshButton.disabled=true;refreshButton.textContent="刷新中…";
    const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),4000);
    try{
        const response=await fetch("/api/status",{cache:"no-store",headers:{Accept:"application/json"},signal:controller.signal});
        if(!response.ok)throw new Error(`状态接口返回 HTTP ${response.status}`);
        const data=await response.json();
        if(data.status!=="running"||data.name!=="EndlessVibe"||typeof data.version!=="string"||typeof data.listen_address!=="string"||!Number.isSafeInteger(data.uptime_seconds)||data.uptime_seconds<0||!Number.isSafeInteger(data.started_at_unix_seconds)||!Number.isSafeInteger(data.checked_at_unix_seconds))throw new Error("状态接口返回了无法识别的数据");
        $("version").textContent=`v${data.version}`;$("uptime").textContent=formatUptime(data.uptime_seconds);$("listen").textContent=data.listen_address;$("started").textContent=formatTime(data.started_at_unix_seconds);
        $("mcp-endpoint").textContent=`http://${window.location.hostname}:${data.port}${data.mcp?.endpoint??"/mcp"}`;
        $("tool-count").textContent=Array.isArray(data.mcp?.tools)?data.mcp.tools.length:"—";$("workspace-count").textContent=data.workspace_count??"—";$("project-count").textContent=data.project_count??"—";$("execution-backend").textContent=data.security?.execution_backend??"—";$("active-jobs").textContent=data.active_jobs??"—";
        $("last-success").textContent=`最近成功检查：${formatTime(data.checked_at_unix_seconds)}`;
        setConnection(true,"当前浏览器已成功访问本地 Dashboard。运行信息每 5 秒更新一次。");
        await refreshMetrics();
    }catch(error){
        const message=error.name==="AbortError"?"请求超过 4 秒未完成":(error instanceof SyntaxError?"状态接口未返回有效 JSON":error.message);
        setConnection(false,`${message}。请检查 EndlessVibe 服务；保留的数值仅代表上次成功检查。`);
    }finally{clearTimeout(timeout);busy=false;refreshButton.disabled=false;refreshButton.textContent="立即刷新";}
}
function resizePlots(){for(const [plot,host] of [[activityPlot,$("activity-chart")],[jobsPlot,$("jobs-chart")]])if(plot&&host.clientWidth>0)plot.setSize({width:Math.max(280,Math.floor(host.clientWidth)),height:270});}
$("site-origin").textContent=window.location.origin;
refreshButton.addEventListener("click",refresh);
document.addEventListener("visibilitychange",()=>{if(!document.hidden)refresh();});
window.addEventListener("online",refresh);
if("ResizeObserver"in window)new ResizeObserver(resizePlots).observe(document.querySelector(".chart-grid"));
else window.addEventListener("resize",resizePlots);
setInterval(()=>{if(!document.hidden)refresh();},5000);
refresh();
