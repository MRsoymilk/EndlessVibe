import {$,node,setText,formatTime} from "./common.js";
let configRevision="",configDirty=false,localConfig=null,selected=null,working=false;
let localRole="unassigned",refreshingNodes=false;
let browserDir=".",selectedFilePath=null;
export function apiErrorMessage(response,payload){const error=payload?.error;return [error?.message,payload?.message,typeof error==="string"?error:null,payload?.detail].find(v=>typeof v==="string"&&v.trim())||`HTTP ${response.status} ${response.statusText||"Request failed"}`}
const get=async(url)=>{const r=await fetch(url,{cache:"no-store",headers:{Accept:"application/json"}});const v=await r.json().catch(()=>({}));if(!r.ok)throw new Error(apiErrorMessage(r,v));return v};
const send=async(url,method,data)=>{const r=await fetch(url,{method,cache:"no-store",headers:{"Content-Type":"application/json",Accept:"application/json"},body:data==null?undefined:JSON.stringify(data)});const v=await r.json().catch(()=>({}));if(!r.ok)throw new Error(apiErrorMessage(r,v));return v};
function status(message,error=false){const el=$("node-status");if(el){el.textContent=message;el.className="write-status "+(error?"error":"success")}}
function btn(label,action){const b=node("button","small-button",label);b.type="button";b.addEventListener("click",async()=>{if(working)return;working=true;b.disabled=true;try{await action()}catch(e){status(e.message,true)}finally{working=false;b.disabled=false}});return b}
function line(parent,...parts){const el=node("div","node-line");for(const part of parts){el.append(typeof part==="string"?document.createTextNode(part):part)}parent.append(el);return el}
function clear(id,empty){const root=$(id);if(!root)return null;root.replaceChildren();if(empty)root.append(node("span","node-muted",empty));return root}
function item(name,tag){const wrap=node("article","node-card"),header=node("div","node-card-header"),title=node("strong","",name);header.append(title,node("span","node-tag",tag));wrap.append(header);return wrap}
function grantPicker(container,initial=[]){
 const projects=(localConfig?.workspaces||[]).flatMap(w=>(w.projects||[]).map(p=>({workspace:w.id,project:p.id,config:p})));
 const existing=new Map(initial.map(g=>[g.workspace+"/"+g.project,g]));
 if(!projects.length){
  container.append(node("span","node-muted","尚无本地 Project。可先完成节点配对，之后到 Projects 页面添加并单独授权。"));
  return {collect:()=>[],hasProjects:false};
 }
 const rows=[];
 for(const project of projects){
  const prior=existing.get(project.workspace+"/"+project.project);
  const box=node("label","node-grant"),enable=document.createElement("input");
  enable.type="checkbox";enable.checked=!!prior;
  box.append(enable,document.createTextNode(" "+project.workspace+"/"+project.project));
  const toggles={};
  for(const [key,allowed] of [["read",true],["write",project.config.allow_write],
     ["execute",project.config.allow_exec],["git",project.config.allow_git_commit]]){
   const entry=node("span","node-grant-permission"),check=document.createElement("input");
   check.type="checkbox";check.disabled=!allowed;
   check.checked=!!allowed&&(prior?!!prior[key]:key==="read");
   entry.append(check,document.createTextNode(" "+key));box.append(entry);
   toggles[key]=check;
  }
  container.append(box);rows.push({project,enable,toggles});
 }
 return {collect:()=>{
  const grants=rows.filter(x=>x.enable.checked).map(x=>({
   workspace:x.project.workspace,project:x.project.project,
   read:x.toggles.read.checked,write:x.toggles.write.checked,
   execute:x.toggles.execute.checked,git:x.toggles.git.checked
  }));
  if(grants.some(g=>!g.read&&!g.write&&!g.execute&&!g.git)){
   throw new Error("已勾选的 Project 必须至少授予一种权限；如需撤销，请取消勾选该 Project");
  }
  if(grants.length>32)throw new Error("最多授权 32 个 Project");
  return grants;
 },hasProjects:true};
}
async function decidePair(p,approved){
 await send(approved?"/api/nodes/approve":"/api/nodes/reject","POST",{id:p.id});
 status(approved
  ?"已同意 "+p.name+" 的连接请求；父节点会自动完成配对。当前没有 Project 授权。"
  :"已拒绝 "+p.name+" 的连接请求。");
 await refreshNodes(true);
}
function renderPending(items){
 const visible=items.filter(p=>p.role!=="child"||p.state!=="rejected");
 const root=clear("node-pending-list",visible.length?"":"没有等待处理的配对申请");
 for(const p of visible){
  const outgoing=p.role==="parent";
  const rejected=p.state==="rejected";
  const card=item(p.name,outgoing?"父节点 → 子节点":"子节点 ← 父节点");
  card.classList.add(outgoing?"node-outgoing":"node-incoming");
  line(card,(outgoing?"目标子节点：":"申请父节点：")+p.node_id+" · "+p.endpoint);
  const code=node("div","node-pair-code",p.code);
  code.setAttribute("aria-label","配对验证码 "+p.code);
  card.append(code);
  if(outgoing){
   const info=rejected
    ?"子节点已拒绝本次申请。请重新发起新的连接请求。"
    :"已发送申请，等待子节点核对验证码并点击同意或拒绝；父节点无需再次确认。";
   card.append(node("p","node-muted",info));
   if(!rejected)line(card,"有效期至 "+formatTime(p.expires_at)+" · 自动检测审批结果");
  }else if(!rejected){
   card.append(node("p","node-muted","请与父节点屏幕上的六位验证码核对一致。点击“同意”即建立配对，但不开放任何 Project 权限。"));
   const actions=node("div","node-pair-decisions");
   const deny=btn("拒绝",()=>decidePair(p,false));
   deny.classList.add("node-pair-reject");
   const accept=btn("同意",()=>decidePair(p,true));
   accept.classList.add("node-pair-accept");
   actions.append(deny,accept);
   card.append(actions);
  }
  root.append(card);
 }
}
function renderDiscovered(items){const root=clear("node-discovered-list",items.length?"":"没有发现其他节点。父节点可手动输入局域网 IP:Port。");for(const p of items){const card=item(p.name,p.online?"Online · 未验证":"Offline · 未验证");line(card,p.node_id+" · "+p.address);if(localRole!=="child"&&localRole!=="mixed"){card.append(btn("发起配对",async()=>{const input=$("node-pair-address");if(input)input.value=p.address;await requestPair(p.address)}));}root.append(card)}}
async function remote(nodeId,tool,arguments_={}){const v=await send("/api/nodes/read","POST",{node_id:nodeId,tool,arguments:arguments_});return v}
function detail(title,content){setText("node-remote-title",title);const output=$("node-remote-output");if(output)output.textContent=typeof content==="string"?content:JSON.stringify(content,null,2);const root=$("node-project-detail");if(root)root.hidden=false;}
// Requests are untrusted child-node data: render labels with textContent, never innerHTML.
async function loadRequestHistory(peer,w,p,cursor=null,append=false){
 const query={workspace:w,project:p,limit:20};
 if(cursor)query.cursor=cursor;
 const data=await remote(peer.node_id,"request_history",query);
 if(!selected||selected.node!==peer.node_id||selected.workspace!==w||selected.project!==p)return;
 const panel=$("node-request-history"),list=append?$("node-history-list"):clear("node-history-list");
 if(!panel||!list)return;
 panel.hidden=false;
 const records=Array.isArray(data.requests)?data.requests:[];
 if(!records.length&&!append){
  list.append(node("span","node-muted","没有可显示的新请求记录。旧请求仍可通过 Request status 按 ID 查询。"));
 }
 // Requests are untrusted child-node data: always render with textContent.
 for(const record of records){
  const row=node("div","node-history-row"),meta=node("div","node-history-meta");
  const tag=node("span","node-history-state",String(record.state||"unknown"));tag.dataset.state=String(record.state||"unknown");
  const identity=node("strong","",String(record.request_id||""));
  const subtitle=node("span","node-muted",String(record.tool||"unknown")+" · "+(record.updated?new Date(record.updated*1000).toLocaleString():"time unavailable"));
  meta.append(identity,subtitle);
  const actions=node("div","node-history-actions");
  actions.append(tag);
  if(record.job_status){const jobTag=node("span","node-history-state","Job: "+record.job_status);jobTag.dataset.state=String(record.job_status);actions.append(jobTag);}
  actions.append(btn("Inspect",async()=>detail(w+"/"+p+" · Request "+record.request_id,await remote(peer.node_id,"request_status",{workspace:w,project:p,request_id:record.request_id}))));
  row.append(meta,actions);list.append(row);
 }
 const count=list.querySelectorAll(".node-history-row").length;
 setText("node-history-summary",count+" loaded"+(data.has_more?" · older records available":""));
 if(data.has_more&&data.next_cursor){
  const more=btn("Load older requests",async()=>{
   await loadRequestHistory(peer,w,p,data.next_cursor,true);
   more.remove();
  });
  more.classList.add("node-history-more");
  list.append(more);
 }
}

function isSelectedProject(peer,workspace,project){
 return selected?.node===peer.node_id&&selected.workspace===workspace&&selected.project===project;
}
function parentRemoteDir(directory){
 const parts=directory.replace(/\\/g,"/").replace(/\/+$/,"").split("/");
 parts.pop();
 return parts.join("/")||".";
}
function childEntryPath(directory,name){
 // Treat filenames from the child as untrusted: never turn separators or traversal into links.
 if(typeof name!=="string"||!name||name==="."||name===".."||
    name.includes("/")||name.includes("\\")||name.includes("\0"))return null;
 return directory==="."?name:directory.replace(/[\\/]+$/,"")+"/"+name;
}
async function readRemoteFile(peer,workspace,project,path){
 path=path.trim();
 if(!path||path===".")throw new Error("请先点击文件列表中的文件，或输入相对文件路径（例如 README.md）");
 const input=$("node-remote-path");
 if(input)input.value=path;
 selectedFilePath=path;
 const result=await remote(peer.node_id,"read_file",{workspace,project,path,start_line:1,max_lines:200});
 if(isSelectedProject(peer,workspace,project))detail(workspace+"/"+project+"/"+path,result);
}
async function listRemoteFiles(peer,workspace,project,directory=".",offset=0){
 const path=directory.trim()||".";
 const result=await remote(peer.node_id,"list_directory",{workspace,project,path,offset,limit:100});
 if(!isSelectedProject(peer,workspace,project)||(offset>0&&browserDir!==path))return;
 const browser=$("node-remote-browser"),location=$("node-remote-location");
 const entries=$("node-remote-file-list"),more=$("node-remote-file-more");
 if(!browser||!location||!entries||!more)return;
 if(offset===0){
  browserDir=path;
  selectedFilePath=null;
  const input=$("node-remote-path");if(input)input.value=path;
  entries.replaceChildren();
  location.replaceChildren();
  if(path!==".")location.append(btn("↑ 上一级",()=>listRemoteFiles(peer,workspace,project,parentRemoteDir(path))));
  const pathLabel=node("span","node-file-location-path",path);
  pathLabel.setAttribute("data-i18n-ignore","");
  location.append(node("span","node-muted","目录："),pathLabel,node("span","node-muted"," · "+result.total+" 项"));
  browser.hidden=false;
 }
 for(const entry of Array.isArray(result.entries)?result.entries:[]){
  const fullPath=childEntryPath(path,entry.name);
  if(!fullPath)continue;
  const row=node("div","node-file-entry");
  if(entry.type==="directory"||entry.type==="file"){
   const directoryEntry=entry.type==="directory";
   const control=btn((directoryEntry?"📁 ":"📄 ")+entry.name,()=>directoryEntry
     ?listRemoteFiles(peer,workspace,project,fullPath)
     :readRemoteFile(peer,workspace,project,fullPath));
   control.classList.add("node-file-open");
   control.title=fullPath;
   control.setAttribute("data-i18n-ignore","");
   row.append(control);
  }else{
   const unsupported=node("span","node-file-unsupported","↪ "+entry.name);
   unsupported.setAttribute("data-i18n-ignore","");
   row.append(unsupported);
  }
  if(entry.type==="file"&&Number.isFinite(entry.bytes)&&entry.bytes>=0){
   row.append(node("span","node-file-size",entry.bytes.toLocaleString()+" B"));
  }
  entries.append(row);
 }
 if(offset===0&&!entries.children.length)entries.append(node("span","node-muted","目录为空"));
 more.replaceChildren();
 if(Number.isSafeInteger(result.next_offset)&&result.next_offset>offset)
  more.append(btn("加载更多文件",()=>listRemoteFiles(peer,workspace,project,path,result.next_offset)));
 detail(workspace+"/"+project+" · "+path,result);
}
async function openProject(peer,w,p){
 selected={node:peer.node_id,workspace:w,project:p};
 browserDir=".";
 selectedFilePath=null;
 const browser=$("node-remote-browser");if(browser)browser.hidden=true;
 const input=$("node-remote-path");if(input)input.value=".";
 const historyRoot=$("node-request-history");if(historyRoot)historyRoot.hidden=true;
 if($("node-remote-args"))$("node-remote-args").value=JSON.stringify({program:"git",args:["status"],request_id:crypto.randomUUID()},null,2);
 const root=clear("node-remote-buttons");if(!root)return;
 const args=()=>({workspace:w,project:p});
 root.append(
  btn("Request history",()=>loadRequestHistory(peer,w,p)),
  btn("Request status",async()=>{
   const id=window.prompt("请输入此前远程操作的 request_id");
   if(!id)return;
   detail(w+"/"+p+" · Request "+id,await remote(peer.node_id,"request_status",{...args(),request_id:id.trim()}));
  }),
  btn("Inspect",async()=>detail(w+"/"+p,await remote(peer.node_id,"inspect_project",args()))),
  btn("Git status",async()=>detail(w+"/"+p+" · Git",await remote(peer.node_id,"git_status",args()))),
  btn("List files",()=>{
   const typed=input?.value.trim()||".";
   return listRemoteFiles(peer,w,p,typed===selectedFilePath?browserDir:typed);
  }),
  btn("Read file",()=>readRemoteFile(peer,w,p,input?.value||""))
 );
 detail("Selected "+w+"/"+p,"点击文件列表中的文件即可读取，或输入项目内的相对路径。");
 await listRemoteFiles(peer,w,p,".");
}
async function browse(peer){selected=null;browserDir=".";selectedFilePath=null;const browser=$("node-remote-browser");if(browser)browser.hidden=true;const historyRoot=$("node-request-history");if(historyRoot)historyRoot.hidden=true;const v=await remote(peer.node_id,"list_workspaces",{});const root=$("node-project-detail");if(root)root.hidden=false;const actions=clear("node-remote-buttons");if(!actions)return;for(const workspace of v.workspaces||[]){const projects=await remote(peer.node_id,"list_projects",{workspace:workspace.id});for(const p of projects.projects||[]){actions.append(btn(workspace.id+"/"+p.id,()=>openProject(peer,workspace.id,p.id)))}}detail(peer.name+" · Projects",actions.children.length?"选择左侧 Project 查看详情。":"没有授权可见的 Project")}
async function revoke(peer){if(!window.confirm("撤销节点 "+peer.name+" 的本地配对？两端均须撤销才能完全解除双向信任。"))return;await send("/api/nodes/peers/"+encodeURIComponent(peer.node_id),"DELETE",{});status("本地已撤销 "+peer.name);if(selected?.node===peer.node_id){selected=null;const detailRoot=$("node-project-detail");if(detailRoot)detailRoot.hidden=true;}await refreshNodes()}
async function savePeerGrants(peer,grants){
 if(!peer.grants_revision)throw new Error("缺少授权版本信息，请刷新 Nodes 页面后重试");
 const detail=grants.length?grants.map(g=>g.workspace+"/"+g.project).join(", "):"无（撤销全部 Project 权限）";
 if(!window.confirm("确认更新已配对父节点 "+peer.name+" 的权限？\n\n"+detail+
   "\n\n这是对完整授权列表的替换，将立即生效；新增 Project 不会自动获得权限。"))return;
 await send("/api/nodes/peers/"+encodeURIComponent(peer.node_id)+"/grants","PUT",{
  expected_grants_revision:peer.grants_revision,grants
 });
 status("已更新 "+peer.name+" 的授权（"+grants.length+" 个 Project），无需重新配对或重启");
 await refreshNodes(true);
}
function renderTrusted(items){
 const root=clear("node-trusted-list",items.length?"":"尚无已配对节点");
 for(const p of items){
  const card=item(p.name,p.role==="parent"?"Child (managed)":"Parent (authorized)");
  line(card,p.node_id+" · "+p.endpoint);
  line(card,p.discovery_online?"Recently discovered on LAN":"Not seen in recent LAN discovery · manual connection may still work");
  if(p.role==="parent"){
   card.append(btn("Browse Projects",()=>browse(p)));
  }else{
   const grants=p.grants||[];
   line(card,grants.length?"当前授权："+grants.map(g=>g.workspace+"/"+g.project).join(", "):
      "已配对 · 尚未授权 Project（可随时添加，无需重新配对）");
   const details=node("details","node-grant-details");
   details.append(node("summary","","管理 Project 授权"));
   const list=node("div","node-grant-list"),picker=grantPicker(list,grants);
   details.append(node("p","node-muted","在 Projects 中添加新项目后，回到这里勾选并保存。取消勾选即撤销授权；不会自动授权。"),list);
   if(!picker.hasProjects){
    const link=node("a","node-project-link","前往 Projects 添加 Project");link.href="/projects";details.append(link);
   }
   details.append(btn("保存 Project 授权",()=>savePeerGrants(p,picker.collect())));
   card.append(details);
  }
  card.append(btn("Revoke locally",()=>revoke(p)));
  root.append(card);
 }
}
async function submitRemoteOperation(event){event.preventDefault();if(!selected){status("请先选择配对节点的 Project",true);return;}const tool=$("node-remote-tool").value;let payload;try{payload=JSON.parse($("node-remote-args").value||"{}");if(!payload||Array.isArray(payload)||typeof payload!=="object")throw new Error("Arguments 必须是 JSON object");if((payload.workspace&&payload.workspace!==selected.workspace)||(payload.project&&payload.project!==selected.project))throw new Error("不能更改当前选中的 Workspace/Project");}catch(e){status("远程参数错误："+e.message,true);return;}payload.workspace=selected.workspace;payload.project=selected.project;const requestId=(tool==="run_command"&&typeof payload.request_id==="string"&&payload.request_id.trim())?payload.request_id.trim():crypto.randomUUID();const summary=`${selected.node} / ${selected.workspace}/${selected.project} / ${tool}`;if(!window.confirm(`确认向子节点执行 ${summary}？\n\n远程修改可能继续在子节点运行，即使当前网络连接断开。若执行超时，请先查询原任务状态，不要更换 request_id 重复运行。`))return;const button=event.currentTarget.querySelector("button[type=submit]");button.disabled=true;try{const result=await send("/api/nodes/write","POST",{node_id:selected.node,tool,arguments:payload,confirm:true,request_id:requestId});detail(summary,result);status(`远程操作 ${tool} 已返回结果；request_id=${requestId}，Job 可继续查询`);}catch(e){status(`远程操作未确认：${e.message}；request_id=${requestId}。请使用 Request status 查询，禁止盲目重放。`,true)}finally{button.disabled=false}}
async function requestPair(address){if(localRole==="child"||localRole==="mixed")throw new Error("当前节点不能作为父节点发起请求");const result=await send("/api/nodes/pair","POST",{address});status("已发送申请，验证码："+result.code+"；请让子节点核对后直接点击“同意”或“拒绝”。");await refreshNodes(true);}
function renderTransferMetrics(metrics){
 if(!metrics){
  setText("node-metrics-detail","Transfer 指标暂不可用；不影响节点配对及项目访问");
  return;
 }
 const requests=metrics.request_records||{},calls=metrics.recent_calls||{};
 const number=v=>Number.isFinite(Number(v))?Number(v):0;
 setText("node-metric-total",number(requests.total).toLocaleString());
 setText("node-metric-review",number(requests.review_required).toLocaleString());
 setText("node-metric-completed",number(requests.completed).toLocaleString());
 const completedCalls=number(calls.succeeded)+number(calls.failed)+number(calls.interrupted);
 setText("node-metric-success",completedCalls?Math.round(number(calls.succeeded)/completedCalls*100)+"%":"—");
 const review=$("node-metric-review")?.parentElement;
 if(review)review.dataset.attention=number(requests.review_required)>0?"true":"false";
 setText("node-metrics-detail",`近 24h ${number(calls.total)} 次远程调用 · ${number(calls.failed)} 失败 · ${number(calls.interrupted)} 中断 · ${number(requests.result_expired)} 条过期响应已留存去重标记`);
}
export async function refreshNodes(force=false){
 if(!$("node-identity")||refreshingNodes)return;
 refreshingNodes=true;
 try{
  const [discovery,pending,config,metrics]=await Promise.all([
   get("/api/nodes"),get("/api/nodes/pending"),get("/api/config"),
   get("/api/nodes/metrics").catch(()=>null)
  ]);
  const peers=await get("/api/nodes/peers");
  localConfig=config;
  localRole=discovery.local_role||"unassigned";
  const names={parent:"父节点 · Parent",child:"子节点 · Child",unassigned:"未配对 · 角色待确定",mixed:"角色冲突 · Mixed",unknown:"身份信息不可用"};
  setText("node-role",names[localRole]||names.unknown);
  const roleEl=$("node-role");if(roleEl)roleEl.dataset.role=localRole;
  const pairCard=$("node-pair-card"),childHint=$("node-child-hint");
  if(pairCard)pairCard.hidden=localRole==="child"||localRole==="mixed";
  if(childHint)childHint.hidden=localRole!=="child";
  setText("node-identity",discovery.enabled?discovery.name+" · "+discovery.node_id+" · "+discovery.listen:
   "Transfer disabled · 在 Config 中启用并重启");
  renderDiscovered(discovery.peers||[]);
  renderPending(pending.pending||[]);
  // Do not discard checked permissions while an editor is open.
  if(force||!document.querySelector("#node-trusted-list details.node-grant-details[open]"))
   renderTrusted(peers.peers||[]);
  renderTransferMetrics(metrics);
  setText("node-updated","Updated "+formatTime(Math.floor(Date.now()/1000)));
 }catch(e){status("Nodes 刷新失败："+e.message,true)}
 finally{refreshingNodes=false}
}
export function renderTransferEditor(config){const form=$("transfer-editor");if(!form||configDirty)return;const c=config.transfer||{};for(const key of ["enabled","advertise","discover"])form.elements.namedItem(key).checked=!!c[key];form.elements.namedItem("display_name").value=c.display_name||"EndlessVibe";form.elements.namedItem("listen").value=c.listen||"0.0.0.0:20002";configRevision=config.revision||"";const display=$("config-transfer");if(display){display.replaceChildren();for(const [key,value] of Object.entries(c)){const dt=document.createElement("dt"),dd=document.createElement("dd");dt.textContent=key;dd.textContent=typeof value==="object"?JSON.stringify(value):String(value);display.append(dt,dd);}}}
async function saveTransfer(event){event.preventDefault();const form=event.currentTarget,button=form.querySelector("button[type=submit]");if(!form.reportValidity())return;const payload={expected_revision:configRevision,display_name:form.elements.namedItem("display_name").value.trim(),listen:form.elements.namedItem("listen").value.trim()};for(const key of ["enabled","advertise","discover"])payload[key]=form.elements.namedItem(key).checked;button.disabled=true;try{await send("/api/config/transfer","PUT",payload);configDirty=false;const statusEl=$("transfer-editor-status");if(statusEl){statusEl.textContent="Transfer 配置已保存。重启 EndlessVibe 后生效。";statusEl.className="write-status success";}const cfg=await get("/api/config");renderTransferEditor(cfg)}catch(e){const el=$("transfer-editor-status");if(el){el.textContent="保存失败："+e.message;el.className="write-status error";}}finally{button.disabled=false}}
export function initNodes(){const pairForm=$("node-pair-form");pairForm?.addEventListener("submit",async event=>{event.preventDefault();const address=$("node-pair-address").value.trim();try{await requestPair(address)}catch(e){status(e.message,true)}});$("node-refresh")?.addEventListener("click",refreshNodes);$("node-remote-operation")?.addEventListener("submit",submitRemoteOperation);$("node-remote-tool")?.addEventListener("change",()=>{const tool=$("node-remote-tool")?.value;const template=tool==="run_command"?{program:"git",args:["status"],request_id:"unique-request-id"}:tool==="get_job"||tool==="cancel_job"?{job_id:"child-job-id"}:tool==="get_job_output"?{job_id:"child-job-id",offset:0,limit:8192}:tool==="create_directory"?{path:"new-folder"}:tool==="write_file"?{path:"file.txt",content:"text",expected_sha256:"MISSING"}:tool==="apply_patch"?{path:"file.txt",expected_sha256:"SHA256",edits:[{old_text:"before",new_text:"after",expected_occurrences:1}]}:{paths:["file.txt"],message:"feat: update",expected_head:"HEAD",expected_diff_sha256:"DIFF_HASH"};const input=$("node-remote-args");if(input)input.value=JSON.stringify(template,null,2);});$("transfer-editor")?.addEventListener("submit",saveTransfer);$("transfer-editor")?.addEventListener("input",()=>{configDirty=true;const el=$("transfer-editor-status");if(el){el.textContent="Transfer 设置有未保存的修改";el.className="write-status pending";}});if(location.pathname==="/nodes"){refreshNodes();setInterval(()=>{if(!document.hidden)refreshNodes()},3000);}}
