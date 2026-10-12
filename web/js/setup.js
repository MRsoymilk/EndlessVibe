import {initPreferences,translate} from "./preferences.js";

const $=id=>document.getElementById(id);
const text=(id,value)=>{const el=$(id);if(el)el.textContent=String(value??"—");};
const toggle=(id,hidden)=>{const el=$(id);if(el)el.hidden=hidden;};
let state=null,step=0,requestSequence=0,busy=false;
const drafts={
  connection:{revision:"",dirty:false,conflict:false},
  workspace:{revision:"",dirty:false,conflict:false},
  project:{revision:"",dirty:false,conflict:false}
};

function status(id,message,kind="neutral",detail=""){
  const el=$(id);if(!el)return;
  el.replaceChildren();el.className="write-status "+kind;
  if(message){el.append(document.createTextNode(translate(message)));}
  if(detail){const line=document.createElement("span");line.className="setup-error-detail";line.dataset.i18nIgnore="";line.textContent=" "+detail;el.append(line);}
}
function payloadError(json,fallback,response){
  const err=json?.error&&typeof json.error==="object"?json.error:json||{};
  const detail=err.message||json?.message||fallback;
  return Object.assign(new Error(String(detail)),{status:response.status,code:err.code||null});
}
async function request(url,method="GET",body){
  const response=await fetch(url,{method,cache:"no-store",headers:{Accept:"application/json",...(body?{"Content-Type":"application/json"}:{})},body:body?JSON.stringify(body):undefined});
  const data=await response.json().catch(()=>({}));
  if(!response.ok)throw payloadError(data,`${method} ${url}: HTTP ${response.status}`,response);
  return data;
}
// Do not put arbitrary URLs, credentials or paths into example commands.
// The backend validates the origin independently before persisting it.
function origin(value){
  const input=String(value||"").trim();
  if(!input||/[\\\s?#]/u.test(input))return null;
  try{
    const uri=new URL(input.includes("://")?input:`https://${input}`);
    if(uri.protocol!=="https:"||uri.username||uri.password||uri.pathname!=="/"||uri.search||uri.hash)return null;
    const host=uri.hostname.toLowerCase();
    const reserved=["localhost","local","invalid","test","example","example.com","example.net","example.org"];
    if(!host.includes(".")||host.length>253||/^\d+(?:\.\d+){3}$/.test(host)||
       host.split(".").some(label=>!(/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(label)))||
       reserved.some(name=>host===name||host.endsWith("."+name)))return null;
    return uri.origin;
  }catch{return null;}
}
function validState(){
  return {domain:!!state?.saved?.public_configured,
    roots:Number(state?.workspace_count||0)>0,
    projects:Number(state?.project_count||0)>0};
}
function unlocked(index){
  const v=validState();
  return index===0||(index===1&&v.domain)||(index===2&&v.domain&&v.roots)||
    (index===3&&v.domain&&v.roots&&v.projects);
}
function switchStep(index){
  if(!unlocked(index)||busy)return;
  step=index;
  document.querySelectorAll("[data-wizard-step]").forEach(el=>{el.hidden=Number(el.dataset.wizardStep)!==step;});
  document.querySelectorAll("[data-wizard-target]").forEach(button=>{
    const value=Number(button.dataset.wizardTarget);
    button.disabled=!unlocked(value)||busy;
    button.removeAttribute("aria-current");
    if(value===step)button.setAttribute("aria-current","step");
    button.closest("li").dataset.complete=String(value<step);
  });
  const prev=$("wizard-prev"),next=$("wizard-next");
  prev.disabled=busy||step===0;
  next.hidden=step===3;
  next.disabled=busy||!unlocked(step+1);
  text("wizard-position",translate(`第 ${step+1} 步，共 4 步`));
  const reason=step===0&&!validState().domain?"先保存公网连接域名，才能进入下一步。":
    step===1&&!validState().roots?"先添加至少一个 Workspace，才能继续。":
    step===2&&!validState().projects?"先添加至少一个明确授权的 Project，才能继续。":"";
  status("wizard-navigation-status",reason,reason?"pending":"neutral");
}
function updatePreview(){
  const form=$("setup-connection-form"),o=origin(form.elements.namedItem("public_url").value),
    container=form.elements.namedItem("connection_mode").value==="container",
    port=Number(state?.saved?.port);
  const validPort=Number.isInteger(port)&&port>0&&port<=65535;
  text("setup-endpoint-preview",o?`${o}/mcp`:"填写域名后生成");
  text("setup-upstream",validPort?`http://${container?"host.docker.internal":"127.0.0.1"}:${port}`:"—");
  text("setup-bind-preview",validPort?`${container?"0.0.0.0":"127.0.0.1"}:${port}`:"—");
  text("setup-ngrok-command",o&&validPort?`ngrok http ${port} --url ${o}`:"填写域名后显示命令");
  text("setup-tunnel-port",validPort?port:"—");
  toggle("setup-loopback-help",container);toggle("setup-container-help",!container);
}
function formatPath(path){return String(path||"").replace(/^\\\\\?\\/,"");}
function entry(name,path,meta){
  const row=document.createElement("div");row.className="wizard-entry";
  const main=document.createElement("div"),title=document.createElement("strong"),code=document.createElement("code");
  title.textContent=name;code.textContent=formatPath(path);main.append(title,code);
  const count=document.createElement("span");count.textContent=meta||"";
  row.append(main,count);return row;
}
function renderProjects(workspaces){
  const roots=$("setup-workspace-list"),projects=$("setup-project-list"),select=$("setup-project-workspace");
  const previous=select.value;roots.replaceChildren();projects.replaceChildren();select.replaceChildren();
  for(const workspace of workspaces||[]){
    roots.append(entry(workspace.id,workspace.path,`${workspace.project_count??0} Projects`));
    const option=document.createElement("option");option.value=workspace.id;option.textContent=workspace.id;select.append(option);
    for(const project of workspace.projects||[]){
      const separator=state?.host_os==="windows"?"\\":"/";
      const absolute=String(workspace.path||"").replace(/[\\/]$/,"")+separator+String(project.path||"").replace(/^[\\/]+/,"");
      projects.append(entry(`${workspace.id} / ${project.id}`,absolute,"授权项目"));
    }
  }
  if(!roots.children.length){const empty=document.createElement("p");empty.className="wizard-empty";empty.textContent="暂无 Workspace";roots.append(empty);}
  if(!projects.children.length){const empty=document.createElement("p");empty.className="wizard-empty";empty.textContent="暂无 Project";projects.append(empty);}
  if([...select.options].some(item=>item.value===previous))select.value=previous;
  text("setup-workspace-count",Number(state?.workspace_count||0));
  text("setup-project-count",Number(state?.project_count||0));
}
function render(data,{forceConnection=false,forceWorkspace=false,forceProject=false}={}){
  state=data;
  const saved=data.saved||{},active=data.active||{},savedOrigin=saved.public_configured?origin(saved.public_url):null;
  text("setup-host-os",data.host_os==="windows"?"Windows":data.host_os==="linux"?"Linux":data.host_os==="macos"?"macOS":data.host_os||"—");
  text("setup-saved-url",savedOrigin||"尚未配置");
  text("setup-active-url",active.public_configured?origin(active.public_url):"仅本机");
  text("setup-saved-bind",saved.bind);
  text("setup-active-bind",active.bind);
  text("setup-config-path",data.config_path);
  text("setup-owner-key-path",data.owner_key_path);
  const badge=$("setup-state");
  badge.dataset.state=!savedOrigin?"new":data.requires_restart?"pending":"saved";
  badge.textContent=translate(!savedOrigin?"待配置":data.requires_restart?"待正常启动":"配置已保存");
  text("setup-state-note",!savedOrigin?"配置 HTTPS 域名并保存后，才可以继续。":data.requires_restart?"地址已保存，但初始化模式不启动公网 MCP。完成向导后请正常启动。":"公网配置已保存；初始化模式不会启动 MCP，请按最后一步启动。");
  const override=data.environment_overrides||{},keys=[];
  if(override.bind)keys.push("ENDLESSVIBE_BIND");
  if(override.public_url)keys.push("ENDLESSVIBE_PUBLIC_URL");
  toggle("setup-environment-note",!keys.length&&!data.startup_override_detected);
  toggle("setup-environment-variables",!keys.length);
  text("setup-environment-keys",keys.join(", "));
  const command=data.restart?.start_command||"endlessvibe";
  text("setup-restart-command",command);
  text("setup-restart-note",data.startup_override_detected?"启动参数或环境变量覆盖了保存的配置；启动正常服务前先修正原启动方式。":"按 Ctrl+C 结束本初始化程序，然后以不带 --init 的模式启动服务。");
  text("setup-restart-command-label",data.host_os==="windows"?"在 Windows PowerShell 中执行：":"在本机终端执行：");
  text("setup-workspace-path-hint",data.host_os==="windows"?"填写已存在的 Windows 绝对路径，例如 D:\\project。":"填写已存在的绝对路径，例如 /home/you/project。");
  const field=$("setup-mcp-url");field.value=savedOrigin?`${savedOrigin}/mcp`:"";
  $("setup-copy-mcp").disabled=!savedOrigin;
  text("setup-mcp-note",savedOrigin?"退出 --init 并正常启动后，才能在 ChatGPT 中授权此地址。":"请先在步骤 1 设置公网 HTTPS 域名。");
  if(!drafts.connection.dirty||forceConnection){
    const form=$("setup-connection-form");
    form.elements.namedItem("public_url").value=savedOrigin||"";
    form.elements.namedItem("connection_mode").value=saved.connection_mode==="container"?"container":"loopback";
    Object.assign(drafts.connection,{revision:data.revision||"",dirty:false,conflict:false});
    toggle("setup-connection-conflict",true);
  }
  for(const [name,force] of [["workspace",forceWorkspace],["project",forceProject]]){
    const draft=drafts[name];
    if(!draft.dirty||force){Object.assign(draft,{revision:data.revision||"",dirty:false,conflict:false});toggle(`setup-${name}-conflict`,true);}
    else if(!draft.revision)draft.revision=data.revision||"";
  }
  renderProjects(data.workspaces||[]);
  updatePreview();syncButtons();switchStep(step);
}
function syncButtons(){
  $("setup-save").disabled=busy||!drafts.connection.revision||drafts.connection.conflict;
  $("setup-add-workspace").disabled=busy||!drafts.workspace.revision||drafts.workspace.conflict;
  $("setup-add-project").disabled=busy||!drafts.project.revision||drafts.project.conflict;
  for(const form of ["setup-connection-fields","setup-workspace-fields","setup-project-fields"])$(form).disabled=busy;
  for(const button of ["setup-reload-connection","setup-reload-workspace","setup-reload-project","setup-copy-start","setup-check-restart"])$(button).disabled=busy;
  document.querySelectorAll("[data-wizard-target]").forEach(button=>button.disabled=busy||!unlocked(Number(button.dataset.wizardTarget)));
  $("wizard-prev").disabled=busy||step===0;
  $("wizard-next").disabled=busy||!unlocked(step+1);
}
async function reload(options){
  const sequence=++requestSequence;
  try{
    const data=await request("/api/setup");
    if(sequence!==requestSequence)return;
    render(data,options);
    status("setup-load-status","");
  }catch(error){if(sequence===requestSequence)status("setup-load-status","读取初始化配置失败。","error",error.message);}
}
async function saveConnection(event){
  event.preventDefault();const form=event.currentTarget,draft=drafts.connection;
  if(busy||draft.conflict||!draft.revision||!form.reportValidity())return;
  const entered=form.elements.namedItem("public_url").value.trim(),normalized=origin(entered);
  if(!normalized){status("setup-save-status","请填写公网 HTTPS 根域名，不包含 /mcp、路径或令牌。","error");return;}
  busy=true;syncButtons();++requestSequence;status("setup-save-status","正在保存域名…","pending");
  try{
    const result=await request("/api/setup","PUT",{expected_revision:draft.revision,public_url:entered,connection_mode:form.elements.namedItem("connection_mode").value});
    render(result,{forceConnection:true});
    status("setup-save-status","连接域名已保存。可以进入下一步。","success");
  }catch(error){
    if(error.status===409){draft.conflict=true;toggle("setup-connection-conflict",false);}
    status("setup-save-status",error.status===409?"配置已变化，输入已保留，请重新载入后保存。":"保存连接域名失败。","error",error.message);
  }finally{busy=false;syncButtons();switchStep(step);}
}
async function addWorkspace(event){
  event.preventDefault();const form=event.currentTarget,draft=drafts.workspace;
  if(busy||draft.conflict||!draft.revision||!form.reportValidity())return;
  const id=form.elements.namedItem("id").value.trim(),path=form.elements.namedItem("path").value.trim();
  busy=true;syncButtons();++requestSequence;status("setup-workspace-status","正在登记 Workspace…","pending");
  try{
    await request("/api/workspaces","POST",{expected_revision:draft.revision,id,path});
    draft.dirty=false;form.reset();
    await reload({forceWorkspace:true,forceProject:true});
    status("setup-workspace-status","Workspace 已保存，继续添加 Project。","success");
  }catch(error){
    if(error.status===409){draft.conflict=true;toggle("setup-workspace-conflict",false);}
    status("setup-workspace-status",error.status===409?"配置已变化，输入已保留，请刷新后重试。":"添加 Workspace 失败。","error",error.message);
  }finally{busy=false;syncButtons();switchStep(step);}
}
async function addProject(event){
  event.preventDefault();const form=event.currentTarget,draft=drafts.project;
  if(busy||draft.conflict||!draft.revision||!form.reportValidity())return;
  const f=form.elements,write=f.namedItem("allow_write").checked,exec=f.namedItem("allow_exec").checked,commit=f.namedItem("allow_git_commit").checked;
  if((exec||commit)&&!write){status("setup-project-status","启用命令执行或 Git Commit 时必须先允许写入。","error");return;}
  const body={expected_revision:draft.revision,workspace:f.namedItem("workspace").value,
    project:f.namedItem("project").value.trim()||null,path:f.namedItem("path").value.trim(),
    execution_profile:f.namedItem("execution_profile").value,allow_write:write,allow_exec:exec,allow_git_commit:commit,
    allow_git_mutation:false,allow_git_push:false};
  busy=true;syncButtons();++requestSequence;status("setup-project-status","正在保存 Project 授权…","pending");
  try{
    await request("/api/projects","POST",body);
    draft.dirty=false;form.reset();
    await reload({forceProject:true});
    status("setup-project-status","Project 已保存。可以进入最后一步。","success");
  }catch(error){
    if(error.status===409){draft.conflict=true;toggle("setup-project-conflict",false);}
    status("setup-project-status",error.status===409?"配置已变化，输入已保留，请刷新后重试。":"添加 Project 失败。","error",error.message);
  }finally{busy=false;syncButtons();switchStep(step);}
}
async function copyText(value,field,statusId,label){
  try{
    if(!navigator.clipboard?.writeText)throw new Error("Clipboard unavailable");
    await navigator.clipboard.writeText(value);status(statusId,label,"success");
  }catch{
    if(field){field.focus();field.select();}
    status(statusId,"浏览器不允许自动复制，请手动复制。","pending");
  }
}
function bind(){
  $("setup-connection-form").addEventListener("submit",saveConnection);
  $("setup-connection-form").addEventListener("input",()=>{drafts.connection.dirty=true;updatePreview();status("setup-save-status","有尚未保存的域名修改。","pending");});
  $("setup-connection-form").addEventListener("change",()=>{drafts.connection.dirty=true;updatePreview();});
  $("setup-workspace-form").addEventListener("submit",addWorkspace);
  $("setup-workspace-form").addEventListener("input",()=>{drafts.workspace.dirty=true;});
  $("setup-project-form").addEventListener("submit",addProject);
  $("setup-project-form").addEventListener("input",()=>{drafts.project.dirty=true;});
  $("setup-project-form").addEventListener("change",()=>{drafts.project.dirty=true;});
  $("setup-reload-connection").addEventListener("click",()=>reload({forceConnection:true,forceWorkspace:true,forceProject:true}));
  $("setup-reload-workspace").addEventListener("click",()=>reload({forceWorkspace:true,forceProject:true}));
  $("setup-reload-project").addEventListener("click",()=>reload({forceProject:true}));
  $("setup-copy-mcp").addEventListener("click",()=>copyText($("setup-mcp-url").value,$("setup-mcp-url"),"setup-copy-status","MCP 地址已复制。"));
  $("setup-copy-start").addEventListener("click",()=>copyText($("setup-restart-command").textContent,null,"setup-start-status","启动命令已复制。"));
  $("setup-check-restart").addEventListener("click",async()=>{
    await reload();
    status("setup-final-status","配置已重新读取。请退出 --init 并正常启动，才能验证 ChatGPT OAuth。","pending");
  });
  $("wizard-prev").addEventListener("click",()=>switchStep(Math.max(0,step-1)));
  $("wizard-next").addEventListener("click",()=>switchStep(Math.min(3,step+1)));
  document.querySelectorAll("[data-wizard-target]").forEach(button=>button.addEventListener("click",()=>switchStep(Number(button.dataset.wizardTarget))));
  window.addEventListener("beforeunload",event=>{if(Object.values(drafts).some(d=>d.dirty)){event.preventDefault();event.returnValue="";}});
  document.addEventListener("endlessvibe:preferences",()=>{if(state){render(state);}});
}
initPreferences();
bind();
switchStep(0);
reload();
