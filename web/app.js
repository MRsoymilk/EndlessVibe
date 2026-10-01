"use strict";
const $=id=>document.getElementById(id);
const refreshButton=$("refresh");
let busy=false;
const dateTime=new Intl.DateTimeFormat("zh-CN",{year:"numeric",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",second:"2-digit",hour12:false});
const formatTime=seconds=>dateTime.format(new Date(seconds*1000));
function formatUptime(seconds){const days=Math.floor(seconds/86400),hours=Math.floor(seconds%86400/3600),minutes=Math.floor(seconds%3600/60),secs=seconds%60;return (days?`${days}天 `:"")+`${String(hours).padStart(2,"0")}:${String(minutes).padStart(2,"0")}:${String(secs).padStart(2,"0")}`;}
function setConnection(online,detail){$("status-badge").className=`badge ${online?"online":"offline"}`;$("badge-text").textContent=online?"运行中":"无法连接";$("status-title").textContent=online?"服务运行正常":"暂时无法获取状态";$("status-detail").textContent=detail;document.querySelector(".metrics").classList.toggle("stale",!online);}
async function refresh(){
    if(busy)return;
    busy=true;refreshButton.disabled=true;refreshButton.textContent="刷新中…";
    const controller=new AbortController(),timeout=setTimeout(()=>controller.abort(),4000);
    try{
        const response=await fetch("/api/status",{cache:"no-store",headers:{Accept:"application/json"},signal:controller.signal});
        if(!response.ok)throw new Error(`状态接口返回 HTTP ${response.status}`);
        const data=await response.json();
        if(data.status!=="running"||data.name!=="EndlessVibe"||typeof data.version!=="string"||typeof data.listen_address!=="string"||!Number.isSafeInteger(data.uptime_seconds)||data.uptime_seconds<0||!Number.isSafeInteger(data.started_at_unix_seconds)||!Number.isSafeInteger(data.checked_at_unix_seconds))throw new Error("状态接口返回了无法识别的数据");
        $("version").textContent=`v${data.version}`;$("uptime").textContent=formatUptime(data.uptime_seconds);$("listen").textContent=data.listen_address;$("started").textContent=formatTime(data.started_at_unix_seconds);$("mcp-endpoint").textContent=`${window.location.origin}/mcp`;
        $("tool-count").textContent=Array.isArray(data.mcp?.tools)?data.mcp.tools.length:"—";$("workspace-count").textContent=data.workspace_count??"—";$("execution-backend").textContent=data.security?.execution_backend??"—";$("active-jobs").textContent=data.active_jobs??"—";
        $("last-success").textContent=`最近成功检查：${formatTime(data.checked_at_unix_seconds)}`;
        setConnection(true,"当前浏览器已成功访问服务状态接口。运行信息每 5 秒更新一次。");
    }catch(error){
        const message=error.name==="AbortError"?"请求超过 4 秒未完成":(error instanceof SyntaxError?"状态接口未返回有效 JSON":error.message);
        setConnection(false,`${message}。请检查服务进程、网络或 Tunnel；保留的数值仅代表上次成功检查。`);
    }finally{clearTimeout(timeout);busy=false;refreshButton.disabled=false;refreshButton.textContent="立即刷新";}
}
$("site-origin").textContent=window.location.origin;
refreshButton.addEventListener("click",refresh);
document.addEventListener("visibilitychange",()=>{if(!document.hidden)refresh();});
window.addEventListener("online",refresh);
setInterval(()=>{if(!document.hidden)refresh();},5000);
refresh();
