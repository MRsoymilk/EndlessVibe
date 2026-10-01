use crate::{config::Config,config_edit::{self,AddProjectRequest,UpdateProjectRequest},security::auth::Auth,store::Store,tools::jobs::Jobs,util,workspace::{self,Project,Workspace}};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{collections::BTreeMap,fs::File,future::Future,path::{Path,PathBuf},sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}},time::Instant};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub struct OperationTrace{pub id:Option<i64>,pub started:Instant}
pub struct Runtime{pub config:Arc<Config>,pub db:Arc<Store>,pub auth:Arc<Auth>,pub jobs:Arc<Jobs>,pub workspaces:BTreeMap<String,Arc<Workspace>>,pub shutdown:CancellationToken,pub config_path:PathBuf,active_config_revision:String,dashboard_events:broadcast::Sender<String>,config_edit_lock:Mutex<()>,traffic_requests:AtomicU64,traffic_rx:AtomicU64,traffic_tx:AtomicU64,started:Instant,started_unix:u64,_instance:File}
impl Runtime{
    pub fn new(mut config:Config,config_path:&Path)->Result<Arc<Self>>{
        config.validate()?;util::private_dir(&config.security.data_dir)?;config.security.data_dir=config.security.data_dir.canonicalize()?;
        let instance=util::single_instance(&config.security.data_dir.join("service.lock"))?;
        for sub in ["tmp","backups","git-journal","empty-home","exec-cache"]{util::private_dir(&config.security.data_dir.join(sub))?;}
        let cfg=config_path.canonicalize().unwrap_or_else(|_|config_path.to_owned());
        for mount in &config.execution.readonly_mounts{let source=mount.source.canonicalize()?;if config.security.data_dir.starts_with(&source)||source.starts_with(&config.security.data_dir)||cfg.starts_with(&source){bail!("Sandbox mounts must not expose EndlessVibe config, credentials or state");}}
        let workspaces=workspace::load(&config,config_path)?;let active_config_revision=config_edit::revision(config_path).unwrap_or_default();let config_path=config_path.to_owned();let config=Arc::new(config);let db=Arc::new(Store::open(&config.security.data_dir.join("state.sqlite3"))?);let auth=Arc::new(Auth::new(config.clone(),db.clone())?);let jobs=Jobs::new(db.clone(),config.clone())?;let(dashboard_events,_)=broadcast::channel(256);
        Ok(Arc::new(Self{config,db,auth,jobs,workspaces,shutdown:CancellationToken::new(),config_path,active_config_revision,dashboard_events,config_edit_lock:Mutex::new(()),traffic_requests:AtomicU64::new(0),traffic_rx:AtomicU64::new(0),traffic_tx:AtomicU64::new(0),started:Instant::now(),started_unix:util::now(),_instance:instance}))
    }
    pub fn workspace_root_exact(&self,id:&str)->Result<Arc<Workspace>>{self.workspaces.get(id).cloned().context("WORKSPACE_NOT_AUTHORIZED: call list_workspaces and use an exact configured workspace ID")}
    pub fn workspace_root(&self,id:&str)->Result<Arc<Workspace>>{
        if !id.is_empty() {
            return self.workspace_root_exact(id);
        }

        if self.workspaces.len() == 1 {
            let workspace_id = self.workspaces.keys().next().expect("workspace count checked");
            return self.workspace_root_exact(workspace_id);
        }

        self.workspace_root_exact(id)
    }
    pub fn project_exact(&self,workspace:&str,project:&str)->Result<Arc<Project>>{self.workspace_root(workspace)?.project(project)}
    pub fn project(&self,workspace:&str,project:&str)->Result<Arc<Project>>{
        if !project.is_empty() {
            return self.project_exact(workspace, project);
        }

        // Cached pre-refactor ChatGPT tool schemas sent the project ID
        // through the old `workspace` field and had no `project` property.
        let legacy_project = workspace;
        let mut resolved = None;

        for workspace_id in self.workspaces.keys() {
            if self.project_exact(workspace_id, legacy_project).is_ok() {
                if resolved.is_some() {
                    anyhow::bail!(
                        "PROJECT_AMBIGUOUS: legacy project ID {} exists in more than one workspace; use workspace + project",
                        legacy_project
                    );
                }
                resolved = Some(workspace_id.clone());
            }
        }

        let workspace_id = resolved.ok_or_else(|| anyhow::anyhow!(
            "PROJECT_NOT_AUTHORIZED: no configured project matches legacy project ID {}",
            legacy_project
        ))?;

        self.project_exact(&workspace_id, legacy_project)
    }
    pub async fn wait_for_operations(&self){let until=Instant::now()+std::time::Duration::from_secs(30);loop{let busy=self.workspaces.values().flat_map(|w|w.projects.values()).any(|p|p.lock.try_lock().is_err());if !busy||Instant::now()>=until{break;}tokio::time::sleep(std::time::Duration::from_millis(50)).await;}}
    pub fn body_limit(&self)->usize{self.config.limits.max_file_bytes.saturating_mul(6).saturating_add(65536)}
    pub fn snapshot(&self)->Value{let projects=self.workspaces.values().map(|w|w.projects.len()).sum::<usize>();json!({"status":"running","name":"EndlessVibe","version":env!("CARGO_PKG_VERSION"),"listen_address":self.config.server.bind.to_string(),"port":self.config.server.bind.port(),"uptime_seconds":self.started.elapsed().as_secs(),"started_at_unix_seconds":self.started_unix,"checked_at_unix_seconds":util::now(),"security":{"authentication":"oauth2_pkce","anonymous_private_tools":false,"execution_backend":self.config.execution.backend,"shell_enabled":self.config.execution.allow_shell,"network_enabled":self.config.execution.backend=="host"||self.config.execution.allow_network,"network_isolation":if self.config.execution.backend=="host"{"none_host_permissions"}else if self.config.execution.allow_network{"shared_host_network"}else{"isolated_or_disabled"}},"dashboard":{"enabled":true,"listen_address":"127.0.0.1:20001","url":"http://127.0.0.1:20001/"},"workspace_count":self.workspaces.len(),"project_count":projects,"active_jobs":self.jobs.active_count(),"mcp":{"endpoint":"/mcp","method":"POST","transport":"streamable_http","session_mode":"stateless","protocol_version":"negotiated","implementation":"rmcp","sdk_version":crate::mcp::SDK_VERSION,"chatgpt_registration":"client_specific_not_asserted","tools":crate::mcp::TOOL_NAMES}})}
    pub fn dashboard_config(&self)->Result<Value>{let persisted=Config::load_file(&self.config_path)?;let revision=config_edit::revision(&self.config_path)?;let workspaces=persisted.workspaces.iter().map(|w|json!({"id":w.id,"path":w.path,"project_count":w.projects.len(),"projects":w.projects.iter().map(|p|json!({"workspace":w.id,"id":p.id,"path":w.path.join(&p.path),"allow_write":p.allow_write,"allow_exec":p.allow_exec,"allow_git_commit":p.allow_git_commit,"allow_git_mutation":p.allow_git_mutation,"allow_git_push":false})).collect::<Vec<_>>() })).collect::<Vec<_>>();Ok(json!({"revision":revision,"active_revision":self.active_config_revision,"requires_restart":revision!=self.active_config_revision,"server":{"bind":persisted.server.bind.to_string(),"public_url":persisted.server.public_url,"allowed_hosts":persisted.server.allowed_hosts},"dashboard":{"listen_address":"127.0.0.1:20001","local_only":true},"security":{"authentication":"oauth2_pkce","allow_http_loopback":persisted.security.allow_http_loopback,"access_token_seconds":persisted.security.access_token_seconds,"refresh_token_seconds":persisted.security.refresh_token_seconds,"extra_redirect_uri_count":persisted.security.extra_redirect_uris.len()},"execution":{"backend":persisted.execution.backend,"allow_shell":persisted.execution.allow_shell,"allow_network":persisted.execution.allow_network,"bubblewrap":persisted.execution.bubblewrap,"path":persisted.execution.path,"allowed_programs":persisted.execution.allowed_programs,"readonly_mounts":persisted.execution.readonly_mounts.iter().map(|m|json!({"source":m.source,"target":m.target})).collect::<Vec<_>>(),"memory_limit_mb":persisted.execution.memory_limit_mb,"max_processes":persisted.execution.max_processes},"limits":{"max_file_bytes":persisted.limits.max_file_bytes,"max_read_bytes":persisted.limits.max_read_bytes,"max_output_bytes":persisted.limits.max_output_bytes,"command_timeout_seconds":persisted.limits.command_timeout_seconds,"max_jobs":persisted.limits.max_jobs,"retained_jobs":persisted.limits.retained_jobs,"search_max_files":persisted.limits.search_max_files,"search_max_bytes":persisted.limits.search_max_bytes},"git":{"executable":persisted.git.executable,"author_name":persisted.git.author_name,"author_email":persisted.git.author_email},"mcp":{"tool_count":crate::mcp::TOOL_NAMES.len(),"tools":crate::mcp::TOOL_NAMES},"workspaces":workspaces,"note":"Sensitive credentials and token material are intentionally omitted. Persisted project changes require restarting EndlessVibe before MCP tools use them."}))}
    pub fn dashboard_add_project(&self,request:AddProjectRequest)->Result<Value>{let input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));let workspace=request.workspace.clone();let project=request.project.clone().unwrap_or_default();let operation=self.begin_operation("dashboard_add_project",&workspace,&project,input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let result=config_edit::add_project(&self.config_path,request)?;let value=serde_json::to_value(&result)?;self.publish_dashboard("config_changed",json!({"revision":result.revision,"requires_restart":result.requires_restart}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn dashboard_update_project(&self,workspace:&str,project:&str,request:UpdateProjectRequest)->Result<Value>{let input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));let operation=self.begin_operation("dashboard_update_project",workspace,project,input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let result=config_edit::update_project(&self.config_path,workspace,project,request)?;let value=serde_json::to_value(&result)?;self.publish_dashboard("config_changed",json!({"revision":result.revision,"requires_restart":result.requires_restart}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn begin_operation(&self,tool:&str,workspace:&str,project:&str,input:Value)->OperationTrace{let id=self.db.operation_start(tool,workspace,project,&input).map_err(|error|tracing::warn!(error=%error,tool=%tool,"Could not start operation log")).ok();OperationTrace{id,started:Instant::now()}}
    pub fn finish_operation(&self,operation:OperationTrace,mut result:Result<Value>)->Result<Value>{let duration_ms=operation.started.elapsed().as_millis() as u64;let mut diff=String::new();let(status,output,error)=match &mut result{Ok(value)=>{if let Some(object)=value.as_object_mut(){if let Some(value)=object.remove("_operation_diff"){if let Some(text)=value.as_str(){diff=text.to_owned();}}else if let Some(text)=object.get("diff").and_then(Value::as_str){diff=text.to_owned();}}let mut logged=value.clone();if !diff.is_empty(){if let Some(object)=logged.as_object_mut(){if object.contains_key("diff"){object.insert("diff".into(),json!({"stored_separately":true,"bytes":diff.len()}));}}}("succeeded",Some(logged),String::new())},Err(error)=>("failed",None,format!("{error:#}"))};if let Some(id)=operation.id{if let Err(log_error)=self.db.operation_finish(id,status,duration_ms,output.as_ref(),&diff,&error){tracing::warn!(error=%log_error,operation_id=id,"Could not finish operation log");}else{self.publish_dashboard("operation_created",json!({"seq":id,"status":status,"duration_ms":duration_ms}));}}result}
    pub fn publish_dashboard(&self,event:&str,data:Value){let _=self.dashboard_events.send(json!({"type":event,"time":util::now(),"data":data}).to_string());}
    pub fn subscribe_dashboard(&self)->broadcast::Receiver<String>{self.dashboard_events.subscribe()}
    pub fn record_http_traffic(&self,rx:u64,tx:u64){self.traffic_requests.fetch_add(1,Ordering::Relaxed);self.traffic_rx.fetch_add(rx,Ordering::Relaxed);self.traffic_tx.fetch_add(tx,Ordering::Relaxed);}
    pub fn take_http_traffic(&self)->(u64,u64,u64){(self.traffic_requests.swap(0,Ordering::AcqRel),self.traffic_rx.swap(0,Ordering::AcqRel),self.traffic_tx.swap(0,Ordering::AcqRel))}
    pub fn workspaces_summary(&self)->Value{json!({"workspaces":self.workspaces.values().map(|w|w.summary()).collect::<Vec<_>>()})}
    pub fn projects_for(&self,workspace:&str)->Result<Value>{Ok(self.workspace_root(workspace)?.projects_summary())}
    fn finish(&self,tool:&str,id:&str,result:Result<Value>)->Result<Value>{let outcome=if result.is_ok(){"succeeded"}else{"failed"};let audit=self.db.audit(tool,id,outcome,"");self.publish_dashboard("activity_changed",json!({"tool":tool,"target":id,"outcome":outcome}));match result{Ok(mut value)=>{if audit.is_err(){value["audit_warning"]=json!("Operation completed but final audit record failed; the started record is retained");}Ok(value)},Err(e)=>Err(e)}}
    pub async fn sync_project<F>(self:&Arc<Self>,tool:&'static str,workspace:&str,project:&str,f:F)->Result<Value> where F:FnOnce(Arc<Runtime>,Arc<Project>)->Result<Value>+Send+'static{let p=self.project(workspace,project)?;let key=format!("{workspace}/{project}");let lock=p.lock.clone().try_lock_owned().context("PROJECT_BUSY: another file/Git/command operation is active")?;self.db.audit(tool,&key,"started","")?;self.publish_dashboard("activity_changed",json!({"tool":tool,"target":key,"outcome":"started"}));let rt=self.clone();let result=tokio::task::spawn_blocking(move||{let _lock=lock;f(rt,p)}).await.context("File worker panicked")?;self.finish(tool,&key,result)}
    pub async fn asynchronous_project<F,Fut>(self:&Arc<Self>,tool:&'static str,workspace:&str,project:&str,f:F)->Result<Value> where F:FnOnce(Arc<Runtime>,Arc<Project>)->Fut+Send+'static,Fut:Future<Output=Result<Value>>+Send+'static{let p=self.project(workspace,project)?;let lock=p.lock.clone().try_lock_owned().context("PROJECT_BUSY: another file/Git/command operation is active")?;let key=format!("{workspace}/{project}");self.db.audit(tool,&key,"started","")?;self.publish_dashboard("activity_changed",json!({"tool":tool,"target":key,"outcome":"started"}));let this=self.clone();tokio::spawn(async move{let _lock=lock;let result=f(this.clone(),p).await;this.finish(tool,&key,result)}).await.context("Git worker panicked")?}
}
