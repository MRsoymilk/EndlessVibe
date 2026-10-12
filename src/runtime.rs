use crate::{config::Config,config_edit::{self,AddWorkspaceRequest,UpdateConnectionRequest,AddProjectRequest,UpdateProjectRequest,UpdateReadonlyMountsRequest,UpdateExecutionBackendRequest,UpdateGitRequest,UpdateLimitsRequest,UpdateDockerRequest,UpdateTransferRequest},security::auth::Auth,store::Store,tools::{jobs::Jobs,process,tasks},transfer::TransferManager,util,workspace::{self,Project,Workspace}};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{collections::BTreeMap,fs::File,future::Future,path::{Path,PathBuf},sync::{Arc,Mutex,RwLock,atomic::{AtomicU64,Ordering}},time::Instant};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
fn windows_sandbox_status(config:&Config)->Value{
    json!({
        "job_objects":{
            "active_for_command_jobs":matches!(config.execution.backend.as_str(),"host"|"appcontainer"),
            "resource_limits_only":true,
            "memory_limit_mb":config.execution.memory_limit_mb,
            "max_processes":config.execution.max_processes,
            "filesystem_isolation":false,
            "network_isolation":false,
        },
        "windows_service_update":{
            "enabled":config.security.allow_windows_service_update,
            "exact_project_required":true,
            "candidate_source":"target/windows-stage/debug/endlessvibe.exe",
            "mcp_tools":["windows_service_update_plan","windows_service_update","windows_service_update_status"],
            "native_helper":true,
            "shell_required":false,
            "explicit_confirmation_and_two_digests":true,
            "graceful_shutdown_only":true,
            "rollback_attempt_on_failure":true,
        },
        "appcontainer":{
            "backend_selectable":true,
            "status":if config.execution.backend=="appcontainer"{"active"}else{"available"},
            "command_jobs":{
                "active":config.execution.backend=="appcontainer",
                "isolation":"less_privileged_appcontainer",
                "capability_count":1,
                "capabilities":["registryRead"],
                "network":false,
                "project_access":"per_job_acl_lease",
                "toolchain_access":"read_only",
                "git_metadata_readonly_unless_authorized":true,
                "live_stdout_stderr":true,
                "timeout_and_cancellation":true,
                "whole_job_exit_verified_before_acl_cleanup":true,
                "cleanup_failure_blocks_new_jobs":true,
                "host_fallback":false,
            },
            "fixed_worker_diagnostics_only":true,
            "local_self_check":"--probe-windows-sandbox",
            "production_worker_entrypoint":true,
            "signature_diagnostics":{
                "embedded_winverifytrust_offline":true,
                "catalog_signatures_checked":false,
                "publisher_identity_pinned":false,
                "execution_gate":false,
            },
            "worker_environment":{
                "system_paths_from_native_windows_api":true,
                "host_loader_path_inherited":false,
                "comspec_exposed":false,
                "extra_key_namespace":"ENDLESSVIBE_",
            },
            "fixed_worker_logs":{
                "private_stdout_stderr":true,
                "combined_limit_bytes":crate::tools::windows_appcontainer::MAX_PRODUCTION_LOG_BYTES,
                "cancel_on_violation":true,
                "tamper_detection":true,
                "general_command_capture":false,
            },
            "crash_recovery_journaled":true,
            "trusted_worker_executable":{
                "origin":"running_endlessvibe_executable_only",
                "max_bytes":crate::tools::windows_trusted_worker::MAX_WORKER_EXECUTABLE_BYTES,
                "non_reparse_private_copy":true,
                "single_link_staged_copy":true,
                "sha256_verified_before_and_after":true,
                "pe_dependency_audit":{
                    "amd64_pe32_plus_only":true,
                    "static_module_name_allowlist":true,
                    "reject_delay_load_imports":true,
                    "msvc_runtime_attested_from_system32":true,
                    "runtime_digest_rechecked":true,
                    "actual_loader_module_path_attestation":true,
                    "parent_toolhelp_pre_and_post_operation":true,
                    "mapped_section_backing_path_crosschecked":true,
                    "pinned_file_volume_index_and_timestamps":true,
                    "pinned_image_handle_identity_checked_twice":true,
                    "actual_section_file_id_verified":false,
                    "mapped_section_byte_identity_proven":false,
                    "only_worker_exe_and_system32_direct_dlls":true,
                    "transient_or_future_loads_attested":false,
                    "authenticode_signatures_verified":false,
                },
                "cleanup_verified_before_reporting_success":true,
                "arbitrary_project_executables":false,
            },
            "supported_operation":"hash_input",
            "protocol_version":crate::tools::sandbox_protocol::VERSION,
            "arbitrary_program_execution":true,
        },
        "profile_recovery":{
            "startup_scan_enabled":true,
            "requires_exclusive_service_lock":true,
            "acl_leases_recovered_before_profiles":true,
        },
    })
}
pub struct OperationTrace{pub id:Option<i64>,pub started:Instant,pub tool:String,pub workspace:String,pub project:String,pub task_id:Option<String>,pub stage:Option<String>}
fn service_config_revision(config:&Config)->Result<String>{Ok(util::digest(serde_json::to_vec(&(&config.server,&config.security,&config.limits,&config.execution,&config.git,&config.docker,&config.transfer))?))}
fn reuse_project_locks(next:&mut BTreeMap<String,Arc<Workspace>>,old:&BTreeMap<String,Arc<Workspace>>){for(workspace_id,next_workspace)in next.iter_mut(){let Some(old_workspace)=old.get(workspace_id)else{continue};let Some(next_workspace)=Arc::get_mut(next_workspace)else{continue};for(project_id,next_project)in next_workspace.projects.iter_mut(){let Some(old_project)=old_workspace.projects.get(project_id)else{continue};if old_project.root.path!=next_project.root.path{continue;}if let Some(next_project)=Arc::get_mut(next_project){next_project.lock=old_project.lock.clone();}}}}
fn setup_connection(config:&Config)->Value{
    json!({"public_url":config.server.public_url,"bind":config.server.bind.to_string(),
        "port":config.server.bind.port(),"connection_mode":if config.server.bind.ip().is_loopback(){"loopback"}else{"container"},
        "mcp_endpoint":config.resource(),"public_configured":config.public_connection_configured(),"lan_only":config.server.lan_only})
}
fn setup_command_argument(value:&str)->String{
    if cfg!(windows){format!("'{}'",value.replace('\'',"''"))}else{format!("'{}'",value.replace('\'',"'\"'\"'"))}
}
pub struct Runtime{pub config:Arc<Config>,pub db:Arc<Store>,pub auth:Arc<Auth>,pub jobs:Arc<Jobs>,pub transfer:Arc<TransferManager>,pub workspaces:RwLock<BTreeMap<String,Arc<Workspace>>>,pub shutdown:CancellationToken,pub config_path:PathBuf,active_config_revision:RwLock<String>,active_service_revision:String,retired_projects:Mutex<Vec<Arc<Project>>>,dashboard_events:broadcast::Sender<String>,config_edit_lock:Mutex<()>,traffic_requests:AtomicU64,traffic_rx:AtomicU64,traffic_tx:AtomicU64,started:Instant,started_unix:u64,_instance:File}
impl Runtime{
    pub fn new(mut config:Config,config_path:&Path)->Result<Arc<Self>>{
        config.validate()?;util::private_dir(&config.security.data_dir)?;config.security.data_dir=config.security.data_dir.canonicalize()?;
        let instance=util::single_instance(&config.security.data_dir.join("service.lock"))?;
        let cfg=config_path.canonicalize().unwrap_or_else(|_|config_path.to_owned());
        for mount in &config.execution.readonly_mounts{let source=mount.source.canonicalize()?;if config.security.data_dir.starts_with(&source)||source.starts_with(&config.security.data_dir)||cfg.starts_with(&source){bail!("Sandbox mounts must not expose EndlessVibe config, credentials or state");}}
        // Open and validate current Projects under the instance lock before
        // recovery resolves their manifest-authorized native tools. This does
        // not execute project code or create the Jobs service.
        let workspaces=workspace::load(&config,config_path)?;
        #[cfg(windows)]
        {
            // service.lock prevents a second instance from recovering an
            // AppContainer that still belongs to the running service.
            // A durable prepared/empty state or a changed native boot ID is
            // required before touching abandoned ACLs; service.lock alone
            // cannot prove that a previous process tree finished exiting.
            let proofs=crate::tools::windows_execution_recovery::verify_recovery(&config.security.data_dir)?;
            let recovered_acls=crate::tools::windows_backend::recover_pending(&config,&workspaces,&proofs)?;
            if recovered_acls>0{tracing::warn!(recovered_acls,"Recovered abandoned AppContainer ACL leases before profiles");}
            let journal=crate::tools::windows_recovery::journal_dir(&config.security.data_dir);
            let recovered=crate::tools::windows_recovery::recover_pending(&journal)?;
            if recovered>0{
                tracing::warn!(recovered,"Recovered abandoned AppContainer profiles before accepting jobs");
            }
            // Retain proofs on every earlier failure. Only fully completed
            // ACL and profile recovery may remove the execution records.
            let recovered_executions=proofs.finish()?;
            if recovered_executions>0{tracing::warn!(recovered_executions,"Cleared verified Windows sandbox execution recovery records");}
        }
        for sub in ["tmp","backups","git-journal","empty-home","exec-cache"]{util::private_dir(&config.security.data_dir.join(sub))?;}
        let active_config_revision=config_edit::revision(config_path).unwrap_or_default();let persisted=if config_path.exists(){Config::load_file(config_path)?}else{config.clone()};let active_service_revision=service_config_revision(&persisted)?;let config_path=config_path.to_owned();let config=Arc::new(config);let db=Arc::new(Store::open(&config.security.data_dir.join("state.sqlite3"))?);let auth=Arc::new(Auth::new(config.clone(),db.clone())?);let jobs=Jobs::new(db.clone(),config.clone())?;let transfer=TransferManager::new(config.transfer.clone(),db.clone())?;let(dashboard_events,_)=broadcast::channel(256);
        Ok(Arc::new(Self{config,db,auth,jobs,transfer,workspaces:RwLock::new(workspaces),shutdown:CancellationToken::new(),config_path,active_config_revision:RwLock::new(active_config_revision),active_service_revision,retired_projects:Mutex::new(vec![]),dashboard_events,config_edit_lock:Mutex::new(()),traffic_requests:AtomicU64::new(0),traffic_rx:AtomicU64::new(0),traffic_tx:AtomicU64::new(0),started:Instant::now(),started_unix:util::now(),_instance:instance}))
    }
    fn workspaces_snapshot(&self)->BTreeMap<String,Arc<Workspace>>{self.workspaces.read().unwrap_or_else(|e|e.into_inner()).clone()}
    pub fn projects_snapshot(&self)->Vec<(String,Arc<Project>)>{self.workspaces_snapshot().values().flat_map(|workspace|workspace.projects.values().map(|project|(workspace.config.id.clone(),project.clone()))).collect()}
    pub fn workspace_root_exact(&self,id:&str)->Result<Arc<Workspace>>{self.workspaces.read().unwrap_or_else(|e|e.into_inner()).get(id).cloned().ok_or_else(||crate::error::coded_details("WORKSPACE_NOT_AUTHORIZED",false,"call list_workspaces and use an exact configured workspace ID",json!({"workspace":id})))}
    pub fn workspace_root(&self,id:&str)->Result<Arc<Workspace>>{
        if !id.is_empty() {
            return self.workspace_root_exact(id);
        }

        let single={let workspaces=self.workspaces.read().unwrap_or_else(|e|e.into_inner());if workspaces.len()==1{workspaces.keys().next().cloned()}else{None}};
        if let Some(workspace_id)=single{return self.workspace_root_exact(&workspace_id);}

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

        for(workspace_id,workspace)in self.workspaces_snapshot(){
            if workspace.project(legacy_project).is_ok(){if resolved.is_some(){return Err(crate::error::coded_details("PROJECT_AMBIGUOUS",false,format!("legacy project ID {} exists in more than one workspace; use workspace + project",legacy_project),json!({"project":legacy_project})));}resolved=Some(workspace_id);}
        }

        let workspace_id = resolved.ok_or_else(||crate::error::coded_details("PROJECT_NOT_AUTHORIZED",false,format!("no configured project matches legacy project ID {}",legacy_project),json!({"project":legacy_project})))?;

        self.project_exact(&workspace_id, legacy_project)
    }
    pub async fn wait_for_operations(&self){let until=Instant::now()+std::time::Duration::from_secs(30);loop{let current=self.projects_snapshot();let retired=self.retired_projects.lock().map(|v|v.clone()).unwrap_or_default();let busy=current.iter().map(|(_,p)|p).chain(retired.iter()).any(|p|p.lock.try_lock().is_err());if !busy||Instant::now()>=until{break;}tokio::time::sleep(std::time::Duration::from_millis(50)).await;}}
    pub fn body_limit(&self)->usize{self.config.limits.max_file_bytes.saturating_mul(6).saturating_add(65536)}
    pub fn workspace_project_counts(&self)->(usize,usize){let workspaces=self.workspaces_snapshot();(workspaces.len(),workspaces.values().map(|w|w.projects.len()).sum())}
    fn reload_authorization_inner(&self)->Result<Value>{
        self.jobs.with_authorization_reload(||{
            let persisted=Config::load_file(&self.config_path)?;let service_revision=service_config_revision(&persisted)?;
            if service_revision!=self.active_service_revision{return Err(crate::error::coded("RELOAD_RESTART_REQUIRED",false,"service-level configuration changed; restart EndlessVibe before marking this config revision active"));}
            let mut next=workspace::load(&persisted,&self.config_path)?;let revision=config_edit::revision(&self.config_path)?;
            let old={let mut current=self.workspaces.write().unwrap_or_else(|e|e.into_inner());reuse_project_locks(&mut next,&current);std::mem::replace(&mut *current,next)};
            {let mut retired=self.retired_projects.lock().map_err(|_|anyhow::anyhow!("Retired project table poisoned"))?;retired.retain(|p|p.lock.try_lock().is_err());for project in old.values().flat_map(|w|w.projects.values()){if project.lock.try_lock().is_err()&&!retired.iter().any(|existing|Arc::ptr_eq(&existing.lock,&project.lock)){retired.push(project.clone());}}}
            *self.active_config_revision.write().unwrap_or_else(|e|e.into_inner())=revision.clone();let(workspace_count,project_count)=self.workspace_project_counts();let active_revision=revision.clone();
            Ok(json!({"revision":revision,"active_revision":active_revision,"requires_restart":false,"workspace_count":workspace_count,"project_count":project_count,"reloaded":true}))
        })
    }
    pub fn dashboard_reload_config(&self)->Result<Value>{let operation=self.begin_operation("dashboard_reload_config","","",json!({}));let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let value=self.reload_authorization_inner()?;self.publish_dashboard("config_changed",value.clone());Ok(value)})();self.finish_operation(operation,result)}
    pub fn dashboard_storage_maintenance(&self)->Result<Value>{let operation=self.begin_operation("dashboard_storage_maintenance","","",json!({}));let result=self.db.maintain(self.config.limits.retained_jobs);self.finish_operation(operation,result)}
    pub fn dashboard_storage_health(&self)->Result<Value>{let mut result=crate::storage::snapshot(&self.config.security.data_dir,&self.db,self.config.limits.retained_jobs)?;let active=self.jobs.active_count();result["cleanup"]=json!({"active_jobs":active,"cache_cleanup_available":active==0,"scope":"exec_cache","requires_confirmation":true});Ok(result)}
    pub async fn dashboard_storage_cache_cleanup(self:&Arc<Self>,request:crate::storage::CleanupRequest)->Result<Value>{let operation=self.begin_operation("dashboard_storage_cache_cleanup","","",json!({"scope":request.scope,"confirmed":request.confirmation==crate::storage::EXEC_CACHE_CONFIRM}));let result=async{if request.scope!="exec_cache"||request.confirmation!=crate::storage::EXEC_CACHE_CONFIRM{bail!("Cache cleanup requires the exact exec_cache scope and confirmation token");}let _cache_guard=self.jobs.begin_cache_cleanup()?;let data_dir=self.config.security.data_dir.clone();let result=tokio::task::spawn_blocking(move||crate::storage::clear_execution_cache(&data_dir)).await.context("Cache cleanup worker panicked")??;self.publish_dashboard("storage_changed",json!({"scope":"exec_cache"}));Ok(result)}.await;self.finish_operation(operation,result)}
    pub async fn dashboard_storage_compact(self:&Arc<Self>)->Result<Value>{let operation=self.begin_operation("dashboard_storage_compact","","",json!({"scope":"sqlite"}));let result=async{let _cache_guard=self.jobs.begin_cache_cleanup()?;let maintenance=self.db.maintain(self.config.limits.retained_jobs)?;let db=self.db.clone();let compact=tokio::task::spawn_blocking(move||db.compact_database()).await.context("SQLite compaction worker panicked")??;self.publish_dashboard("storage_changed",json!({"scope":"sqlite"}));Ok(json!({"maintenance":maintenance,"compaction":compact}))}.await;self.finish_operation(operation,result)}
    pub async fn dashboard_sandbox_diagnostics(&self)->Result<Value>{
        let mut diagnostics=process::sandbox_diagnostics(&self.config).await?;
        #[cfg(windows)]
        {
            diagnostics["windows_security"]=windows_sandbox_status(&self.config);
        }
        let persisted_revision=config_edit::revision(&self.config_path)?;
        let active_revision=self.active_config_revision.read().unwrap_or_else(|e|e.into_inner()).clone();
        Ok(json!({
            "generated_at":util::now(),
            "persisted_revision":persisted_revision,
            "active_revision":active_revision,
            "requires_restart":persisted_revision!=active_revision,
            "diagnostics":diagnostics,
        }))
    }
    pub fn snapshot(&self)->Value{let admission=self.jobs.admission_status();let workspaces=self.workspaces_snapshot();let projects=workspaces.values().map(|w|w.projects.len()).sum::<usize>();let development_projects=workspaces.values().flat_map(|w|w.projects.values()).filter(|p|p.config.development()).count();let network_enabled=self.config.execution.backend!="appcontainer"&&(self.config.execution.backend=="host"||self.config.execution.allow_network||development_projects>0);json!({"status":"running","name":"EndlessVibe","version":env!("CARGO_PKG_VERSION"),"listen_address":self.config.server.bind.to_string(),"port":self.config.server.bind.port(),"uptime_seconds":self.started.elapsed().as_secs(),"started_at_unix_seconds":self.started_unix,"checked_at_unix_seconds":util::now(),"security":{"authentication":"oauth2_pkce","anonymous_private_tools":false,"execution_backend":self.config.execution.backend,"shell_enabled":self.config.execution.allow_shell,"network_enabled":network_enabled,"development_project_count":development_projects,"network_mode":if self.config.execution.backend=="host"{"host_always"}else if self.config.execution.backend=="appcontainer"{"appcontainer_offline"}else if development_projects>0{"project_profiles"}else if self.config.execution.allow_network{"per_job_opt_in"}else{"isolated"},"network_isolation":if self.config.execution.backend=="host"{"none_host_permissions"}else if self.config.execution.backend=="appcontainer"{"LPAC with zero network capabilities, including development projects"}else if development_projects>0{"isolated projects stay isolated; development projects share host network by default"}else if self.config.execution.allow_network{"isolated_by_default; network=true shares host network"}else{"isolated_or_disabled"}},"dashboard":{"enabled":true,"listen_address":"127.0.0.1:20001","url":"http://127.0.0.1:20001/"},"workspace_count":workspaces.len(),"project_count":projects,"active_jobs":self.jobs.active_count(),"accepting_jobs":admission["accepting_jobs"],"job_pause_reason":admission["pause_reason"],"transfer":{"enabled":self.transfer.config.enabled,"node_id":self.transfer.node_id,"listen":self.transfer.config.listen},"docker":{"enabled":self.config.docker.enabled,"container_count":self.config.docker.allowed_containers.len(),"project_socket_enabled":self.config.docker.allow_project_socket},"mcp":{"endpoint":"/mcp","method":"POST","transport":"streamable_http","session_mode":"stateless","protocol_version":"negotiated","implementation":"rmcp","sdk_version":crate::mcp::SDK_VERSION,"tool_schema_revision":crate::mcp::TOOL_SCHEMA_REVISION,"tool_count":crate::mcp::TOOL_NAMES.len(),"chatgpt_registration":"client_specific_not_asserted","tools":crate::mcp::TOOL_NAMES}})}
    pub fn dashboard_setup(&self)->Result<Value>{
        // Content and revision must describe one file snapshot even during another Dashboard save.
        let bytes=std::fs::read(&self.config_path).context("Read setup configuration")?;
        let mut persisted:Config=toml::from_str(std::str::from_utf8(&bytes).context("Config must be UTF-8")?).context("Invalid config.toml")?;
        persisted.server.public_url=persisted.server.public_url.trim_end_matches('/').to_owned();persisted.validate()?;
        let revision=util::digest(&bytes);
        let active_revision=self.active_config_revision.read().unwrap_or_else(|e|e.into_inner()).clone();
        let matches_active=persisted.server.public_url==self.config.server.public_url
            &&persisted.server.bind==self.config.server.bind&&persisted.server.lan_only==self.config.server.lan_only
            &&persisted.security.allow_http_loopback==self.config.security.allow_http_loopback;
        let file_pending=revision!=active_revision;
        let startup_override_detected=!matches_active&&service_config_revision(&persisted)?==self.active_service_revision;
        let config_path=if self.config_path.is_absolute(){self.config_path.clone()}else{std::env::current_dir()?.join(&self.config_path)};
        let executable=std::env::current_exe().unwrap_or_else(|_|PathBuf::from(if cfg!(windows){"endlessvibe.exe"}else{"endlessvibe"}));
        let start_command=format!("{}{} --config {}",if cfg!(windows){"& "}else{""},
            setup_command_argument(&executable.to_string_lossy()),setup_command_argument(&config_path.to_string_lossy()));
        let restart_supported=cfg!(any(windows,target_os="linux"));
        let command=if restart_supported{format!("{start_command} --restart")}else{String::new()};
        let workspaces=self.workspaces_snapshot();let projects=workspaces.values().map(|w|w.projects.len()).sum::<usize>();
        Ok(json!({"revision":revision,"requires_restart":file_pending||!matches_active,
            "needs_setup":!persisted.server.lan_only&&!persisted.public_connection_configured(),
            "host_os":std::env::consts::OS,"config_path":config_path,"owner_key_path":self.config.owner_key_path(),
            "workspace_count":workspaces.len(),"project_count":projects,
            "saved":setup_connection(&persisted),"active":setup_connection(&self.config),
            "startup_override_detected":startup_override_detected,"connection_matches_saved":matches_active,
            "environment_overrides":{"bind":std::env::var_os("ENDLESSVIBE_BIND").is_some(),"public_url":std::env::var_os("ENDLESSVIBE_PUBLIC_URL").is_some()},
            "restart":{"supported":restart_supported,"command":command,"start_command":start_command,"dashboard_url":"http://127.0.0.1:20001/setup"},
            "public_reachability":"not_checked","chatgpt_connection":"not_verified"}))
    }
    pub fn dashboard_update_connection(&self,mut request:UpdateConnectionRequest)->Result<Value>{
        request.public_url=crate::config::normalize_connection_url(&request.public_url)?;
        let operation=self.begin_operation("dashboard_update_connection","","",json!({
            "expected_revision":request.expected_revision,"public_url":request.public_url,"connection_mode":request.connection_mode}));
        let result=(||{
            let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;
            let saved=config_edit::update_connection(&self.config_path,request)?;
            let mut value=self.dashboard_setup()?;value["_operation_diff"]=saved["_operation_diff"].clone();
            self.publish_dashboard("config_changed",json!({"revision":value["revision"],"requires_restart":value["requires_restart"]}));Ok(value)
        })();self.finish_operation(operation,result)
    }
    pub fn dashboard_add_workspace(&self,request:AddWorkspaceRequest)->Result<Value>{
        let operation=self.begin_operation("dashboard_add_workspace",&request.id,"",serde_json::to_value(&request)?);
        let result=(||{
            let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;
            let mut value=config_edit::add_workspace(&self.config_path,request)?;
            match self.reload_authorization_inner(){Ok(_)=>value["requires_restart"]=json!(false),Err(error)=>{
                value["requires_restart"]=json!(true);value["reload_warning"]=json!(format!("{error:#}"));}}
            self.publish_dashboard("config_changed",json!({"revision":value["revision"],"requires_restart":value["requires_restart"]}));Ok(value)
        })();self.finish_operation(operation,result)
    }
    pub fn dashboard_config(&self)->Result<Value>{let persisted=Config::load_file(&self.config_path)?;let revision=config_edit::revision(&self.config_path)?;let active_revision=self.active_config_revision.read().unwrap_or_else(|e|e.into_inner()).clone();let effective=workspace::load(&persisted,&self.config_path)?;let workspaces=effective.values().map(|w|json!({"id":w.config.id,"path":w.root.path,"project_count":w.projects.len(),"projects":w.projects.values().map(|p|p.summary()).collect::<Vec<_>>() })).collect::<Vec<_>>();let requires_restart=revision!=active_revision;Ok(json!({"revision":revision,"active_revision":active_revision,"requires_restart":requires_restart,"server":{"bind":persisted.server.bind.to_string(),"public_url":persisted.server.public_url,"allowed_hosts":persisted.server.allowed_hosts},"dashboard":{"listen_address":"127.0.0.1:20001","local_only":true},"security":{"authentication":"oauth2_pkce","allow_http_loopback":persisted.security.allow_http_loopback,"access_token_seconds":persisted.security.access_token_seconds,"refresh_token_seconds":persisted.security.refresh_token_seconds,"extra_redirect_uri_count":persisted.security.extra_redirect_uris.len()},"execution":{"backend":persisted.execution.backend,"active_backend":self.config.execution.backend,"host_os":std::env::consts::OS,"allow_shell":persisted.execution.allow_shell,"allow_network":persisted.execution.allow_network,"auto_discover_toolchains":persisted.execution.auto_discover_toolchains,"bubblewrap":persisted.execution.bubblewrap,"path":persisted.execution.path,"allowed_programs":persisted.execution.allowed_programs,"required_programs":persisted.execution.required_programs,"readonly_mounts":persisted.execution.readonly_mounts.iter().map(|m|json!({"source":m.source,"target":m.target})).collect::<Vec<_>>(),"memory_limit_mb":persisted.execution.memory_limit_mb,"max_processes":persisted.execution.max_processes},"limits":{"max_file_bytes":persisted.limits.max_file_bytes,"max_read_bytes":persisted.limits.max_read_bytes,"max_output_bytes":persisted.limits.max_output_bytes,"command_timeout_seconds":persisted.limits.command_timeout_seconds,"max_jobs":persisted.limits.max_jobs,"retained_jobs":persisted.limits.retained_jobs,"search_max_files":persisted.limits.search_max_files,"search_max_bytes":persisted.limits.search_max_bytes},"git":{"executable":persisted.git.executable,"author_name":persisted.git.author_name,"author_email":persisted.git.author_email},"transfer":{"enabled":persisted.transfer.enabled,"listen":persisted.transfer.listen,"advertise":persisted.transfer.advertise,"discover":persisted.transfer.discover,"display_name":persisted.transfer.display_name},"docker":{"enabled":persisted.docker.enabled,"socket":persisted.docker.socket,"allowed_containers":persisted.docker.allowed_containers,"allow_start":persisted.docker.allow_start,"allow_stop":persisted.docker.allow_stop,"allow_restart":persisted.docker.allow_restart,"allow_project_socket":persisted.docker.allow_project_socket},"mcp":{"tool_schema_revision":crate::mcp::TOOL_SCHEMA_REVISION,"tool_count":crate::mcp::TOOL_NAMES.len(),"tools":crate::mcp::TOOL_NAMES},"workspaces":workspaces,"note":"Sensitive credentials and token material are intentionally omitted. Project authorization changes can be hot-reloaded; service-level changes still require restart."}))}
    pub fn dashboard_update_execution_backend(&self,request:UpdateExecutionBackendRequest)->Result<Value>{
        let operation=self.begin_operation("dashboard_update_execution_backend","","",
            json!({"expected_revision":request.expected_revision,"backend":request.backend,
                "host_acknowledged":request.acknowledge_unsafe_host_execution}));
        let result=(||{
            let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;
            let value=config_edit::update_execution_backend(&self.config_path,request)?;
            self.publish_dashboard("config_changed",json!({"revision":value["revision"],"requires_restart":true}));
            Ok(value)
        })();
        self.finish_operation(operation,result)
    }
    pub fn dashboard_update_readonly_mounts(&self,request:UpdateReadonlyMountsRequest)->Result<Value>{let input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));let operation=self.begin_operation("dashboard_update_readonly_mounts","","",input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let result=config_edit::update_readonly_mounts(&self.config_path,request)?;let value=serde_json::to_value(&result)?;self.publish_dashboard("config_changed",json!({"revision":result.revision,"requires_restart":true}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn dashboard_update_transfer(&self,request:UpdateTransferRequest)->Result<Value>{let input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));let op=self.begin_operation("dashboard_update_transfer","","",input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let value=config_edit::update_transfer(&self.config_path,request)?;self.publish_dashboard("config_changed",json!({"revision":value["revision"],"requires_restart":true}));Ok(value)})();self.finish_operation(op,result)}
    pub fn dashboard_update_docker(&self,request:UpdateDockerRequest)->Result<Value>{let mut input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));if let Some(map)=input.as_object_mut(){map.remove("acknowledge_daemon_control");}let operation=self.begin_operation("dashboard_update_docker","","",input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let value=config_edit::update_docker(&self.config_path,request)?;self.publish_dashboard("config_changed",json!({"revision":value["revision"],"requires_restart":true}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn dashboard_update_git(&self,request:UpdateGitRequest)->Result<Value>{let operation=self.begin_operation("dashboard_update_git","","",json!({"expected_revision":request.expected_revision,"executable":request.executable,"author_name":request.author_name,"author_email":request.author_email}));let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let value=config_edit::update_git(&self.config_path,request)?;self.publish_dashboard("config_changed",json!({"revision":value["revision"],"requires_restart":true}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn dashboard_update_limits(&self,request:UpdateLimitsRequest)->Result<Value>{let input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));let operation=self.begin_operation("dashboard_update_limits","","",input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let value=config_edit::update_limits(&self.config_path,request)?;self.publish_dashboard("config_changed",json!({"revision":value["revision"],"requires_restart":true}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn dashboard_add_project(&self,request:AddProjectRequest)->Result<Value>{let input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));let workspace=request.workspace.clone();let project=request.project.clone().unwrap_or_default();let operation=self.begin_operation("dashboard_add_project",&workspace,&project,input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let mut result=config_edit::add_project(&self.config_path,request)?;match self.reload_authorization_inner(){Ok(_)=>result.requires_restart=false,Err(error)=>{result.requires_restart=true;result.reload_warning=Some(format!("{error:#}"));}}let value=serde_json::to_value(&result)?;self.publish_dashboard("config_changed",json!({"revision":result.revision,"requires_restart":result.requires_restart}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn dashboard_update_project(&self,workspace:&str,project:&str,request:UpdateProjectRequest)->Result<Value>{let input=serde_json::to_value(&request).unwrap_or_else(|_|json!({}));let operation=self.begin_operation("dashboard_update_project",workspace,project,input);let result=(||{let _guard=self.config_edit_lock.lock().map_err(|_|anyhow::anyhow!("Config edit lock poisoned"))?;let mut result=config_edit::update_project(&self.config_path,workspace,project,request)?;match self.reload_authorization_inner(){Ok(_)=>result.requires_restart=false,Err(error)=>{result.requires_restart=true;result.reload_warning=Some(format!("{error:#}"));}}let value=serde_json::to_value(&result)?;self.publish_dashboard("config_changed",json!({"revision":result.revision,"requires_restart":result.requires_restart}));Ok(value)})();self.finish_operation(operation,result)}
    pub fn begin_operation(&self,tool:&str,workspace:&str,project:&str,input:Value)->OperationTrace{let task_id=input.get("task_id").and_then(Value::as_str).map(str::to_owned);let stage=input.get("stage").and_then(Value::as_str).map(str::to_owned);let id=self.db.operation_start(tool,workspace,project,&input).map_err(|error|tracing::warn!(error=%error,tool=%tool,"Could not start operation log")).ok();OperationTrace{id,started:Instant::now(),tool:tool.into(),workspace:workspace.into(),project:project.into(),task_id,stage}}
    pub fn finish_operation(&self,operation:OperationTrace,mut result:Result<Value>)->Result<Value>{let duration_ms=operation.started.elapsed().as_millis() as u64;let mut diff=String::new();let(status,output,error_code,error)=match &mut result{Ok(value)=>{if let Some(object)=value.as_object_mut(){if let Some(value)=object.remove("_operation_diff"){if let Some(text)=value.as_str(){diff=text.to_owned();}}else if let Some(text)=object.get("diff").and_then(Value::as_str){diff=text.to_owned();}}let mut logged=value.clone();if operation.tool=="docker_logs"{if let Some(object)=logged.as_object_mut(){if let Some(logs)=object.get("logs").and_then(Value::as_str){let redacted=json!({"omitted":true,"bytes":logs.len(),"sha256":util::digest(logs)});object.insert("logs".into(),redacted);}}}if operation.tool=="node_write"||operation.tool=="node_dashboard_write"||operation.tool=="node_read"||operation.tool.starts_with("transfer_"){let body=value.to_string();logged=json!({"redacted":true,"bytes":body.len(),"sha256":util::digest(&body)});diff.clear();}if !diff.is_empty(){if let Some(object)=logged.as_object_mut(){if object.contains_key("diff"){object.insert("diff".into(),json!({"stored_separately":true,"bytes":diff.len()}));}}}("succeeded",Some(logged),"",String::new())},Err(error)=>("failed",None,crate::error::code(error),format!("{error:#}"))};if let Some(id)=operation.id{if let Err(log_error)=self.db.operation_finish(id,status,duration_ms,output.as_ref(),&diff,error_code,&error){tracing::warn!(error=%log_error,operation_id=id,"Could not finish operation log");}else{self.publish_dashboard("operation_created",json!({"seq":id,"status":status,"error_code":error_code,"duration_ms":duration_ms}));let task=operation.task_id.as_deref().or_else(||result.as_ref().ok().and_then(|v|v.get("task_id")).and_then(Value::as_str));let stage=operation.stage.as_deref().or_else(||result.as_ref().ok().and_then(|v|v.get("stage")).and_then(Value::as_str));if let(Some(task),Some(stage))=(task,stage){if tasks::validate_context(&Some(task.into()),&Some(stage.into())).is_ok(){let workspace=result.as_ref().ok().and_then(|v|v.get("canonical_workspace")).and_then(Value::as_str).unwrap_or(&operation.workspace);let project=result.as_ref().ok().and_then(|v|v.get("canonical_project")).and_then(Value::as_str).unwrap_or(&operation.project);if let Ok(p)=self.project(workspace,project){if let Err(error)=tasks::record_operation(&self.db,&p.workspace_id,&p.config.id,task,stage,id,&operation.tool,status){tracing::warn!(error=%error,operation_id=id,"Could not link operation to task");}}}}}}result}
    pub fn publish_dashboard(&self,event:&str,data:Value){let _=self.dashboard_events.send(json!({"type":event,"time":util::now(),"data":data}).to_string());}
    pub fn subscribe_dashboard(&self)->broadcast::Receiver<String>{self.dashboard_events.subscribe()}
    pub fn record_http_traffic(&self,rx:u64,tx:u64){self.traffic_requests.fetch_add(1,Ordering::Relaxed);self.traffic_rx.fetch_add(rx,Ordering::Relaxed);self.traffic_tx.fetch_add(tx,Ordering::Relaxed);}
    pub fn take_http_traffic(&self)->(u64,u64,u64){(self.traffic_requests.swap(0,Ordering::AcqRel),self.traffic_rx.swap(0,Ordering::AcqRel),self.traffic_tx.swap(0,Ordering::AcqRel))}
    pub fn workspaces_summary(&self)->Value{let workspaces=self.workspaces_snapshot();json!({"workspaces":workspaces.values().map(|w|w.summary()).collect::<Vec<_>>()})}
    pub fn projects_for(&self,workspace:&str)->Result<Value>{Ok(self.workspace_root(workspace)?.projects_summary())}
    fn finish(&self,tool:&str,id:&str,result:Result<Value>)->Result<Value>{let outcome=if result.is_ok(){"succeeded"}else{"failed"};let audit=self.db.audit(tool,id,outcome,"");self.publish_dashboard("activity_changed",json!({"tool":tool,"target":id,"outcome":outcome}));match result{Ok(mut value)=>{if audit.is_err(){value["audit_warning"]=json!("Operation completed but final audit record failed; the started record is retained");}Ok(value)},Err(e)=>Err(e)}}
    pub async fn sync_project<F>(self:&Arc<Self>,tool:&'static str,workspace:&str,project:&str,f:F)->Result<Value> where F:FnOnce(Arc<Runtime>,Arc<Project>)->Result<Value>+Send+'static{let p=self.project(workspace,project)?;let key=format!("{workspace}/{project}");let lock=p.lock.clone().try_lock_owned().map_err(|_|crate::error::coded_details("PROJECT_BUSY",true,"another file/Git/command operation is active",json!({"workspace":workspace,"project":project})))?;self.db.audit(tool,&key,"started","")?;self.publish_dashboard("activity_changed",json!({"tool":tool,"target":key,"outcome":"started"}));let rt=self.clone();let result=tokio::task::spawn_blocking(move||{let _lock=lock;f(rt,p)}).await.context("File worker panicked")?;self.finish(tool,&key,result)}
    pub async fn asynchronous_project<F,Fut>(self:&Arc<Self>,tool:&'static str,workspace:&str,project:&str,f:F)->Result<Value> where F:FnOnce(Arc<Runtime>,Arc<Project>)->Fut+Send+'static,Fut:Future<Output=Result<Value>>+Send+'static{let p=self.project(workspace,project)?;let lock=p.lock.clone().try_lock_owned().map_err(|_|crate::error::coded_details("PROJECT_BUSY",true,"another file/Git/command operation is active",json!({"workspace":workspace,"project":project})))?;let key=format!("{workspace}/{project}");self.db.audit(tool,&key,"started","")?;self.publish_dashboard("activity_changed",json!({"tool":tool,"target":key,"outcome":"started"}));let this=self.clone();tokio::spawn(async move{let _lock=lock;let result=f(this.clone(),p).await;this.finish(tool,&key,result)}).await.context("Git worker panicked")?}
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn service_revision_ignores_workspace_authorization(){let a=Config::default();let mut b=a.clone();b.workspaces.push(crate::config::WorkspaceConfig{id:"root".into(),path:"/tmp/root".into(),projects:vec![],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]});assert_eq!(service_config_revision(&a).unwrap(),service_config_revision(&b).unwrap());b.execution.allow_shell=!b.execution.allow_shell;assert_ne!(service_config_revision(&a).unwrap(),service_config_revision(&b).unwrap());let mut c=a.clone();c.docker.allow_project_socket=true;assert_ne!(service_config_revision(&a).unwrap(),service_config_revision(&c).unwrap());}
    #[test]fn reload_reuses_project_locks(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let root=t.path().join("root");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(root.join("app")).unwrap();let cfg_path=t.path().join("config.toml");std::fs::write(&cfg_path,"").unwrap();let mut cfg=Config::default();cfg.security.data_dir=state;cfg.workspaces=vec![crate::config::WorkspaceConfig{id:"root".into(),path:root,projects:vec![crate::config::ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:crate::config::default_project_profile(),environment:vec![]}],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];let old=workspace::load(&cfg,&cfg_path).unwrap();let mut next=workspace::load(&cfg,&cfg_path).unwrap();assert!(!Arc::ptr_eq(&old["root"].projects["app"].lock,&next["root"].projects["app"].lock));reuse_project_locks(&mut next,&old);assert!(Arc::ptr_eq(&old["root"].projects["app"].lock,&next["root"].projects["app"].lock));}
}

#[cfg(all(windows,test))]
mod windows_sandbox_status_tests {
    use super::*;
    #[test]
    fn appcontainer_capability_and_active_backend_are_reported_separately(){
        let mut config=Config::default();
        config.execution.backend="host".into();
        let reported=windows_sandbox_status(&config);
        assert_eq!(reported["job_objects"]["active_for_command_jobs"],true);
        assert_eq!(reported["job_objects"]["filesystem_isolation"],false);
        assert_eq!(reported["job_objects"]["network_isolation"],false);
        assert_eq!(reported["appcontainer"]["backend_selectable"],true);
        assert_eq!(reported["appcontainer"]["command_jobs"]["active"],false);
        assert_eq!(reported["windows_service_update"]["enabled"],false);
        assert_eq!(reported["windows_service_update"]["native_helper"],true);
        assert_eq!(reported["windows_service_update"]["explicit_confirmation_and_two_digests"],true);
        assert_eq!(reported["windows_service_update"]["shell_required"],false);
        assert_eq!(reported["appcontainer"]["status"],"available");
        assert_eq!(reported["appcontainer"]["production_worker_entrypoint"],true);
        assert_eq!(reported["appcontainer"]["signature_diagnostics"]["embedded_winverifytrust_offline"],true);
        assert_eq!(reported["appcontainer"]["signature_diagnostics"]["catalog_signatures_checked"],false);
        assert_eq!(reported["appcontainer"]["signature_diagnostics"]["publisher_identity_pinned"],false);
        assert_eq!(reported["appcontainer"]["signature_diagnostics"]["execution_gate"],false);
        assert_eq!(reported["appcontainer"]["worker_environment"]["system_paths_from_native_windows_api"],true);
        assert_eq!(reported["appcontainer"]["worker_environment"]["host_loader_path_inherited"],false);
        assert_eq!(reported["appcontainer"]["worker_environment"]["comspec_exposed"],false);
        assert_eq!(reported["appcontainer"]["worker_environment"]["extra_key_namespace"],"ENDLESSVIBE_");
        assert_eq!(reported["appcontainer"]["fixed_worker_logs"]["combined_limit_bytes"],65536);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["max_bytes"],134217728);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["sha256_verified_before_and_after"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["cleanup_verified_before_reporting_success"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["arbitrary_project_executables"],false);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["reject_delay_load_imports"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["msvc_runtime_attested_from_system32"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["actual_loader_module_path_attestation"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["parent_toolhelp_pre_and_post_operation"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["mapped_section_backing_path_crosschecked"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["pinned_file_volume_index_and_timestamps"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["pinned_image_handle_identity_checked_twice"],true);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["actual_section_file_id_verified"],false);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["mapped_section_byte_identity_proven"],false);
        assert_eq!(reported["appcontainer"]["trusted_worker_executable"]["pe_dependency_audit"]["transient_or_future_loads_attested"],false);
        assert_eq!(reported["appcontainer"]["fixed_worker_logs"]["tamper_detection"],true);
        assert_eq!(reported["appcontainer"]["fixed_worker_logs"]["cancel_on_violation"],true);
        assert_eq!(reported["appcontainer"]["fixed_worker_logs"]["general_command_capture"],false);
        assert_eq!(reported["appcontainer"]["crash_recovery_journaled"],true);
        assert_eq!(reported["appcontainer"]["arbitrary_program_execution"],true);
        config.execution.backend="appcontainer".into();
        let active=windows_sandbox_status(&config);
        assert_eq!(active["appcontainer"]["status"],"active");
        assert_eq!(active["appcontainer"]["command_jobs"]["active"],true);
        assert_eq!(active["appcontainer"]["command_jobs"]["capability_count"],1);
        assert_eq!(active["appcontainer"]["command_jobs"]["capabilities"],json!(["registryRead"]));
        assert_eq!(active["appcontainer"]["command_jobs"]["network"],false);
        assert_eq!(active["appcontainer"]["command_jobs"]["host_fallback"],false);
        assert_eq!(reported["profile_recovery"]["startup_scan_enabled"],true);
        config.execution.backend="disabled".into();
        assert_eq!(windows_sandbox_status(&config)["job_objects"]["active_for_command_jobs"],false);
    }
}
