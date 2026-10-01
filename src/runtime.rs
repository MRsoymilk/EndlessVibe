use crate::{config::Config,security::auth::Auth,store::Store,tools::jobs::Jobs,util,workspace::{self,Workspace}};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{collections::BTreeMap,fs::File,future::Future,path::Path,sync::Arc,time::Instant};
use tokio_util::sync::CancellationToken;

pub struct Runtime{pub config:Arc<Config>,pub db:Arc<Store>,pub auth:Arc<Auth>,pub jobs:Arc<Jobs>,pub workspaces:BTreeMap<String,Arc<Workspace>>,pub shutdown:CancellationToken,started:Instant,started_unix:u64,_instance:File}
impl Runtime{
    pub fn new(mut config:Config,config_path:&Path)->Result<Arc<Self>>{
        config.validate()?;util::private_dir(&config.security.data_dir)?;config.security.data_dir=config.security.data_dir.canonicalize()?;
        let instance=util::single_instance(&config.security.data_dir.join("service.lock"))?;
        for sub in ["tmp","backups","git-journal","empty-home","exec-cache"]{util::private_dir(&config.security.data_dir.join(sub))?;}
        let cfg=config_path.canonicalize().unwrap_or_else(|_|config_path.to_owned());
        for mount in &config.execution.readonly_mounts{let source=mount.source.canonicalize()?;if config.security.data_dir.starts_with(&source)||source.starts_with(&config.security.data_dir)||cfg.starts_with(&source){bail!("Sandbox mounts must not expose EndlessVibe config, credentials or state");}}
        let workspaces=workspace::load(&config,config_path)?;let config=Arc::new(config);let db=Arc::new(Store::open(&config.security.data_dir.join("state.sqlite3"))?);let auth=Arc::new(Auth::new(config.clone(),db.clone())?);let jobs=Jobs::new(db.clone(),config.clone())?;
        Ok(Arc::new(Self{config,db,auth,jobs,workspaces,shutdown:CancellationToken::new(),started:Instant::now(),started_unix:util::now(),_instance:instance}))
    }
    pub fn workspace(&self,id:&str)->Result<Arc<Workspace>>{self.workspaces.get(id).cloned().context("WORKSPACE_NOT_AUTHORIZED: call list_projects and use an exact configured ID")}
    pub async fn wait_for_operations(&self){let until=Instant::now()+std::time::Duration::from_secs(30);loop{let busy=self.workspaces.values().any(|w|w.lock.try_lock().is_err());if !busy||Instant::now()>=until{break;}tokio::time::sleep(std::time::Duration::from_millis(50)).await;}}
    pub fn body_limit(&self)->usize{self.config.limits.max_file_bytes.saturating_mul(6).saturating_add(65536)}
    pub fn snapshot(&self)->Value{json!({"status":"running","name":"EndlessVibe","version":env!("CARGO_PKG_VERSION"),"listen_address":self.config.server.bind.to_string(),"port":self.config.server.bind.port(),"uptime_seconds":self.started.elapsed().as_secs(),"started_at_unix_seconds":self.started_unix,"checked_at_unix_seconds":util::now(),"security":{"authentication":"oauth2_pkce","anonymous_private_tools":false,"execution_backend":self.config.execution.backend,"shell_enabled":self.config.execution.allow_shell,"network_enabled":self.config.execution.backend=="host"||self.config.execution.allow_network,"network_isolation":if self.config.execution.backend=="host"{"none_host_permissions"}else if self.config.execution.allow_network{"shared_host_network"}else{"isolated_or_disabled"}},"workspace_count":self.workspaces.len(),"active_jobs":self.jobs.active_count(),"mcp":{"endpoint":"/mcp","method":"POST","transport":"streamable_http","session_mode":"stateless","protocol_version":"negotiated","implementation":"rmcp","sdk_version":crate::mcp::SDK_VERSION,"chatgpt_registration":"client_specific_not_asserted","tools":crate::mcp::TOOL_NAMES}})}
    pub fn projects(&self)->Value{json!({"projects":self.workspaces.values().map(|w|w.summary()).collect::<Vec<_>>()})}
    fn finish(&self,tool:&str,id:&str,result:Result<Value>)->Result<Value>{
        let audit=self.db.audit(tool,id,if result.is_ok(){"succeeded"}else{"failed"},"");
        match result{Ok(mut value)=>{if audit.is_err(){value["audit_warning"]=json!("Operation completed but final audit record failed; the started record is retained");}Ok(value)},Err(e)=>Err(e)}
    }
    pub async fn sync<F>(self:&Arc<Self>,tool:&'static str,id:&str,f:F)->Result<Value> where F:FnOnce(Arc<Runtime>,Arc<Workspace>)->Result<Value>+Send+'static{
        let w=self.workspace(id)?;let lock=w.lock.clone().try_lock_owned().context("WORKSPACE_BUSY: another file/Git/command operation is active")?;self.db.audit(tool,id,"started","")?;
        let rt=self.clone();let result=tokio::task::spawn_blocking(move||{let _lock=lock;f(rt,w)}).await.context("File worker panicked")?;self.finish(tool,id,result)
    }
    pub async fn asynchronous<F,Fut>(self:&Arc<Self>,tool:&'static str,id:&str,f:F)->Result<Value> where F:FnOnce(Arc<Runtime>,Arc<Workspace>)->Fut+Send+'static,Fut:Future<Output=Result<Value>>+Send+'static{
        let w=self.workspace(id)?;let lock=w.lock.clone().try_lock_owned().context("WORKSPACE_BUSY: another file/Git/command operation is active")?;self.db.audit(tool,id,"started","")?;
        let this=self.clone();let id=id.to_owned();
        // A dropped HTTP request must not interrupt a partially published Git transaction.
        tokio::spawn(async move{let _lock=lock;let result=f(this.clone(),w).await;this.finish(tool,&id,result)}).await.context("Git worker panicked")?
    }
}
