import {$,setText,node} from "./common.js";
import {translate} from "./preferences.js";

let getJson,sendJson,currentPage="home",state=null,requestSequence=0,mutationBusy=false;
let connectionRevision="",connectionDirty=false,connectionConflict=false;
let workspaceRevision="",workspaceDirty=false,workspaceConflict=false;

// Only a plain HTTPS origin can enter copied commands or the MCP address preview.
// The server performs the authoritative domain and configuration validation.
export function setupPublicOrigin(value){
  const text=String(value||"").trim();
  if(!text||/[\\\s?#]/u.test(text))return null;
  try{
    const url=new URL(text.includes("://")?text:`https://${text}`);
    if(url.protocol!=="https:"||url.username||url.password||url.pathname!=="/"||url.search||url.hash)return null;
    const labels=url.hostname.split(".");
    if(url.hostname.length>253||labels.length<2||labels.some(label=>!(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/i.test(label))))return null;
    if(/^\d+(?:\.\d+){3}$/.test(url.hostname)||url.hostname.endsWith(".localhost"))return null;
    return url.origin;
  }catch{return null;}
}

function status(id,message,kind="neutral",detail=""){
  const element=$(id);if(!element)return;
  element.className=`write-status ${kind}`;
  element.replaceChildren(node("span","",message));
  if(detail){const description=node("span","setup-error-detail",String(detail));description.dataset.i18nIgnore="";element.append(description);}
}
function setHidden(id,hidden){const element=$(id);if(element)element.hidden=hidden;}
function fields(id){return $(id)?.elements;}
function input(name){return fields("setup-connection-form")?.namedItem(name);}
function setBusy(busy){
  mutationBusy=busy;
  for(const id of ["setup-connection-fields","setup-workspace-fields","setup-refresh","setup-check-restart","setup-reload-connection","setup-reload-workspace"]){const element=$(id);if(element)element.disabled=busy;}
  syncSubmitButtons();
}
function syncSubmitButtons(){
  if($("setup-save"))$("setup-save").disabled=mutationBusy||!connectionRevision||connectionConflict;
  if($("setup-add-workspace"))$("setup-add-workspace").disabled=mutationBusy||!workspaceRevision||workspaceConflict;
}
function publicAddress(config){return config?.public_configured?setupPublicOrigin(config.public_url):null;}
function connectionChanged(data){
  return typeof data.connection_matches_saved==="boolean"?!data.connection_matches_saved:data.saved?.public_url!==data.active?.public_url||data.saved?.bind!==data.active?.bind;
}
function renderBanner(data){
  const show=!!data.needs_setup||(!!data.requires_restart&&connectionChanged(data));
  setHidden("setup-home-banner",!show);
  setText("setup-home-title",data.needs_setup?"将本机项目连接到 ChatGPT":"连接配置已保存，等待重启");
  setText("setup-home-description",data.needs_setup?"按照向导设置域名、连接 ChatGPT，再选择允许访问的项目。":"打开连接向导，按提示重启并核对当前运行的地址。");
}
function renderSummary(data){
  const saved=publicAddress(data.saved),active=publicAddress(data.active);
  setText("setup-saved-url",saved||translate(data.saved?.lan_only?"仅局域网":"尚未设置公网域名"));
  setText("setup-active-url",active||translate(data.active?.lan_only?"仅局域网":"尚未启用公网连接"));
  setText("setup-saved-bind",data.saved?.bind||"—");
  setText("setup-active-bind",data.active?.bind||"—");
  setText("setup-config-path",data.config_path||"—");
  setText("setup-owner-key-path",data.owner_key_path||"—");
  setText("setup-host-os",({windows:"Windows",linux:"Linux",macos:"macOS"})[data.host_os]||data.host_os||"—");
  setText("setup-workspace-count",data.workspace_count??0);
  setText("setup-project-count",data.project_count??0);
  const badge=$("setup-state");
  if(badge){badge.dataset.state=data.requires_restart||data.startup_override_detected?"pending":saved?"saved":"new";badge.textContent=translate(data.startup_override_detected?"启动参数覆盖":data.requires_restart?"待重启":saved?"配置已生效":"待配置");}
  setText("setup-state-note",data.startup_override_detected?"当前启动参数覆盖了已保存地址。请先调整原启动命令或环境变量，再重启并核对。":data.requires_restart?"服务配置已更改。重启后，请确认“当前运行地址”与“已保存地址”一致。":saved?"服务已加载当前配置。公网是否可达，以及 ChatGPT 是否完成授权，需要在连接时确认。":"先准备一个公网 HTTPS 域名，再保存下方连接设置。");
  const overrides=data.environment_overrides||{},names=[];
  if(overrides.bind)names.push("ENDLESSVIBE_BIND");
  if(overrides.public_url)names.push("ENDLESSVIBE_PUBLIC_URL");
  setHidden("setup-environment-note",!names.length&&!data.startup_override_detected);
  setHidden("setup-environment-variables",!names.length);
  setText("setup-environment-keys",names.join(", "));
  const endpoint=saved?`${saved}/mcp`:"";
  if($("setup-mcp-url"))$("setup-mcp-url").value=endpoint;
  if($("setup-copy-mcp"))$("setup-copy-mcp").disabled=!endpoint;
  setText("setup-mcp-note",!saved?"保存公网域名后，这里会生成 MCP 地址。":data.requires_restart?"这是已保存的 MCP 地址。请先重启 EndlessVibe，再到 ChatGPT 完成连接。":"复制此地址到 ChatGPT。保存配置不代表公网已可达或 ChatGPT 已连接。");
  setText("setup-restart-command",data.restart?.command||data.restart?.start_command||translate("请使用当前安装方式停止并重新启动 EndlessVibe。"));
  setText("setup-restart-command-label",data.restart?.supported===false?"先在原终端按 Ctrl+C 或通过服务管理器停止，再运行：":"在本机终端执行：");
  setText("setup-restart-note",data.startup_override_detected?"请先移除或更新覆盖网页设置的启动参数和环境变量，再通过原来的启动方式重启。":data.requires_restart?"当前有尚未生效的配置。请在运行 EndlessVibe 的本机终端或服务管理器中重启。":"当前配置无需重启。修改域名或连接方式并保存后，再按这里的提示操作。");
  setText("setup-workspace-path-hint",data.host_os==="windows"?"填写已存在的 Windows 绝对路径，例如 D:\\project。":"填写已存在的绝对路径，例如 /home/you/project。");
}
function render(data,{reloadConnection=false,reloadWorkspace=false}={}){
  state=data;
  renderBanner(data);renderSummary(data);
  if(!connectionDirty||reloadConnection){
    input("public_url").value=publicAddress(data.saved)||"";
    input("connection_mode").value=data.saved?.connection_mode==="container"?"container":"loopback";
    connectionRevision=data.revision||"";connectionDirty=false;connectionConflict=false;
    setHidden("setup-connection-conflict",true);
  }else if(!connectionRevision)connectionRevision=data.revision||"";
  if(!workspaceDirty||reloadWorkspace){workspaceRevision=data.revision||"";workspaceConflict=false;setHidden("setup-workspace-conflict",true);}
  else if(!workspaceRevision)workspaceRevision=data.revision||"";
  syncPreview();syncSubmitButtons();
}
function syncPreview(){
  if(!state)return;
  const origin=setupPublicOrigin(input("public_url")?.value),container=input("connection_mode")?.value==="container",port=Number(state.saved?.port);
  const validPort=Number.isInteger(port)&&port>0&&port<65536;
  setText("setup-endpoint-preview",origin?`${origin}/mcp`:translate("填写域名后生成"));
  setText("setup-upstream",validPort?`http://${container?"host.docker.internal":"127.0.0.1"}:${port}`:"—");
  setText("setup-bind-preview",validPort?`${container?"0.0.0.0":"127.0.0.1"}:${port}`:"—");
  setHidden("setup-container-help",!container);
  setHidden("setup-loopback-help",container);
  setText("setup-ngrok-command",origin&&validPort?`ngrok http ${port} --url ${origin}`:translate("填写公网域名后显示 ngrok 命令。"));
  setText("setup-tunnel-port",validPort?String(port):"—");
}

export async function refreshSetup(options={}){
  if(!getJson)return false;
  const sequence=++requestSequence;
  try{
    const data=await getJson("/api/setup");
    if(sequence!==requestSequence)return false;
    render(data,options);status("setup-load-status","");
    return true;
  }catch(error){
    if(sequence===requestSequence)status("setup-load-status","暂时无法读取配置。帮助内容仍可查看，服务恢复后请重新检查。","error",error.message);
    return false;
  }
}
async function saveConnection(event){
  event.preventDefault();const form=event.currentTarget;
  if(mutationBusy||connectionConflict||!connectionRevision||!form.reportValidity())return;
  const publicUrl=String(input("public_url").value||"").trim();
  if(!setupPublicOrigin(publicUrl)){status("setup-save-status","请填写裸域名或 HTTPS 根地址；不要包含 /mcp、其他路径、账号、查询参数或片段。","error");input("public_url").focus();return;}
  const body={expected_revision:connectionRevision,public_url:publicUrl,connection_mode:input("connection_mode").value};
  ++requestSequence;connectionDirty=true;setBusy(true);status("setup-save-status","正在验证并保存连接设置…","pending");
  try{
    const data=await sendJson("/api/setup","PUT",body);
    ++requestSequence;render(data,{reloadConnection:true});
    status("setup-save-status",data.requires_restart?"连接设置已保存。请完成下方的重启步骤。":"连接设置已保存，当前配置已生效。请继续到 ChatGPT 完成授权。",data.requires_restart?"pending":"success");
  }catch(error){
    if(error.status===409){connectionConflict=true;setHidden("setup-connection-conflict",false);status("setup-save-status","配置已发生变化，当前输入已保留。请加载最新已保存配置后重新编辑。","error");}
    else status("setup-save-status",error.status?"未能保存连接设置。":"保存结果尚未确认。请检查当前配置后再决定是否重试。","error",error.message);
  }finally{setBusy(false);}
}
async function addWorkspace(event){
  event.preventDefault();const form=event.currentTarget;
  if(mutationBusy||workspaceConflict||!workspaceRevision||!form.reportValidity())return;
  const id=form.elements.namedItem("id").value.trim(),path=form.elements.namedItem("path").value.trim();
  if(!id||!path){status("setup-workspace-status","请填写管理根名称和已存在的绝对路径。","error");return;}
  ++requestSequence;workspaceDirty=true;setBusy(true);status("setup-workspace-status","正在添加管理根…","pending");
  try{
    const result=await sendJson("/api/workspaces","POST",{expected_revision:workspaceRevision,id,path});
    workspaceDirty=false;workspaceConflict=false;workspaceRevision=result.revision||workspaceRevision;
    form.elements.namedItem("id").value="";form.elements.namedItem("path").value="";
    status("setup-workspace-status",result.requires_restart?"管理根已保存，重启后生效。接下来可在 Projects 中添加授权项目。":"管理根已添加。接下来在 Projects 中选择具体项目并设置权限。",result.requires_restart?"pending":"success",result.reload_warning||"");
    await refreshSetup();
  }catch(error){
    if(error.status===409){workspaceConflict=true;setHidden("setup-workspace-conflict",false);status("setup-workspace-status","配置已发生变化，管理根输入已保留。请刷新配置后重试。","error");}
    else status("setup-workspace-status",error.status?"未能添加管理根。":"添加结果尚未确认。请在 Projects 核对已有管理根后再决定是否重试。","error",error.message);
  }finally{setBusy(false);}
}
async function copyMcp(){
  const field=$("setup-mcp-url"),value=field?.value;
  if(!value)return;
  try{if(!navigator.clipboard?.writeText)throw new Error("clipboard unavailable");await navigator.clipboard.writeText(value);status("setup-copy-status","MCP 地址已复制。","success");}
  catch{field.focus();field.select();status("setup-copy-status","浏览器未允许复制，地址已选中，请手动复制。","pending");}
}
export function initSetup(api){
  getJson=api.getJson;sendJson=api.sendJson;currentPage=api.currentPage;
  $("setup-connection-form")?.addEventListener("submit",saveConnection);
  $("setup-connection-form")?.addEventListener("input",()=>{connectionDirty=true;syncPreview();if(!connectionConflict)status("setup-save-status","连接设置有未保存的修改。","pending");});
  $("setup-connection-form")?.addEventListener("change",()=>{connectionDirty=true;syncPreview();if(!connectionConflict)status("setup-save-status","连接设置有未保存的修改。","pending");});
  $("setup-workspace-form")?.addEventListener("submit",addWorkspace);
  $("setup-workspace-form")?.addEventListener("input",()=>{workspaceDirty=true;if(!workspaceConflict)status("setup-workspace-status","管理根信息尚未保存。","pending");});
  $("setup-refresh")?.addEventListener("click",()=>refreshSetup());
  $("setup-check-restart")?.addEventListener("click",async()=>{if(await refreshSetup())status("setup-load-status",state.startup_override_detected?"启动参数仍在覆盖网页配置，请先调整启动命令或环境变量。":state.requires_restart?"服务仍有待重启的配置，请核对启动方式和环境变量。":"已重新读取当前运行配置。公网连接仍需在 ChatGPT 中验证。",state.requires_restart||state.startup_override_detected?"pending":"success");});
  $("setup-reload-connection")?.addEventListener("click",async()=>{if(await refreshSetup({reloadConnection:true}))status("setup-save-status","已加载最新已保存配置，请重新编辑后保存。","neutral");});
  $("setup-reload-workspace")?.addEventListener("click",async()=>{if(await refreshSetup({reloadWorkspace:true}))status("setup-workspace-status","已读取最新配置，管理根输入已保留，可以重新提交。","neutral");});
  $("setup-copy-mcp")?.addEventListener("click",copyMcp);
  document.addEventListener("endlessvibe:preferences",()=>{if(state){renderSummary(state);renderBanner(state);syncPreview();}});
  window.addEventListener("beforeunload",event=>{if(currentPage==="setup"&&(connectionDirty||workspaceDirty)){event.preventDefault();event.returnValue="";}});
}
