use anyhow::{bail,Context,Result};
use endlessvibe::{config::{self,Config,ProjectConfig,WorkspaceConfig},runtime::Runtime,security::auth::Auth,server,store::Store,tools::process,util,workspace};
use std::{os::unix::fs::MetadataExt,path::{Path,PathBuf},sync::Arc};

#[derive(Default)]struct Cli{config:Option<PathBuf>,init:bool,workspaces:Vec<String>,add_projects:Vec<String>,public_url:Option<String>,bind:Option<String>,read_only:bool,no_exec:bool,allow_shell:bool,unsafe_host:bool,check_sandbox:bool,issue_token:bool,revoke_all:bool,rotate_key:bool,audit:bool,status:bool,stop:bool,restart:bool}
fn arguments()->Result<Option<Cli>>{
    let mut c=Cli::default();let mut args=std::env::args().skip(1);
    while let Some(arg)=args.next(){match arg.as_str(){
        "--help"|"-h"=>{println!("EndlessVibe {}\n\nInitialize a workspace root once:\n  endlessvibe --init --workspace project=/home/user/project\n\nMount projects later (service must be stopped):\n  endlessvibe --add-project project:EndlessVibe=/home/user/project/EndlessVibe\n  endlessvibe --add-project project=/home/user/project/BAfter\n\nOptions:\n  --config PATH                 Configuration file (default: XDG config/endlessvibe/config.toml)\n  --init                        Create a new configuration and an owner key; never overwrite config\n  --workspace ID=/ABS/PATH      Create a workspace root at initialization; repeat for multiple roots\n  --add-project WORKSPACE[:ID]=/ABS/PATH  Mount a project below an existing workspace; ID defaults to directory name\n  --public-url HTTPS_ORIGIN     Public origin, without /mcp\n  --bind IP:PORT                Override bind address (default 0.0.0.0:20000)\n  --read-only                   Make projects added by --add-project read-only\n  --no-exec                     Disable command execution for projects added by --add-project\n  --allow-shell                 Initialize with run_shell enabled (broad execution permission)\n  --unsafe-host-exec            EXPLICIT opt-in: run commands with host-user permissions, no sandbox\n  --check-sandbox               Probe bubblewrap plus configured allowed/required programs\n  --issue-token                 Print a short-lived local diagnostic bearer token; keep private\n  --revoke-all                  Revoke all access/refresh tokens and pending authorizations\n  --rotate-owner-key            Rotate owner key and revoke tokens; service must be stopped\n  --audit-tail                  Print recent private audit events\n  --status                      Show whether the configured service instance is running\n  --stop                        Gracefully stop the configured service instance\n  --restart                     Gracefully stop it, then start this binary as the new instance\n  --version                     Print version\n\nDefault execution uses bubblewrap with networking disabled.\nChatGPT endpoint: /mcp; select OAuth, not No Authentication.\n",env!("CARGO_PKG_VERSION"));return Ok(None);},
        "--version"|"-V"=>{println!("EndlessVibe {}",env!("CARGO_PKG_VERSION"));return Ok(None);},
        "--config"=>c.config=Some(PathBuf::from(args.next().context("--config needs a path")?)),
        "--workspace"=>c.workspaces.push(args.next().context("--workspace needs ID=/absolute/path")?),
        "--add-project"=>c.add_projects.push(args.next().context("--add-project needs WORKSPACE[:PROJECT]=/absolute/path")?),
        "--public-url"=>c.public_url=Some(args.next().context("--public-url needs an origin")?),
        "--bind"=>c.bind=Some(args.next().context("--bind needs IP:port")?),
        "--init"=>c.init=true,"--read-only"=>c.read_only=true,"--no-exec"=>c.no_exec=true,"--allow-shell"=>c.allow_shell=true,"--unsafe-host-exec"=>c.unsafe_host=true,"--check-sandbox"=>c.check_sandbox=true,"--issue-token"=>c.issue_token=true,"--revoke-all"=>c.revoke_all=true,"--rotate-owner-key"=>c.rotate_key=true,"--audit-tail"=>c.audit=true,"--status"=>c.status=true,"--stop"=>c.stop=true,"--restart"=>c.restart=true,
        _=>bail!("Unknown argument {arg}; use --help"),
    }}Ok(Some(c))
}
enum ServiceState{Stopped,Running(util::ProcessIdentity),LegacyRunning}
fn service_state(settings:&Config)->Result<ServiceState>{
    util::private_dir(&settings.security.data_dir)?;
    if let Some(identity)=util::read_service_identity(&settings.security.data_dir)?{if util::process_identity_alive(identity){return Ok(ServiceState::Running(identity));}util::clear_service_identity(&settings.security.data_dir,identity)?;}
    match util::single_instance(&settings.security.data_dir.join("service.lock")){Ok(_)=>Ok(ServiceState::Stopped),Err(_)=>Ok(ServiceState::LegacyRunning)}
}
async fn stop_service(settings:&Config)->Result<bool>{
    let identity=match service_state(settings)?{ServiceState::Stopped=>{println!("EndlessVibe is not running");return Ok(false);},ServiceState::LegacyRunning=>bail!("A running EndlessVibe instance holds service.lock but has no service.pid. It predates built-in process control; stop it once using the previous method, then future --stop/--restart commands will work."),ServiceState::Running(identity)=>identity};
    if unsafe{libc::kill(identity.pid,libc::SIGTERM)}!=0{let error=std::io::Error::last_os_error();if error.raw_os_error()==Some(libc::ESRCH){util::clear_service_identity(&settings.security.data_dir,identity)?;println!("EndlessVibe is not running (removed stale service.pid)");return Ok(false);}return Err(error).context("Send SIGTERM to EndlessVibe");}
    let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(15);while util::process_identity_alive(identity)&&tokio::time::Instant::now()<deadline{tokio::time::sleep(std::time::Duration::from_millis(50)).await;}
    if util::process_identity_alive(identity){bail!("EndlessVibe PID {} did not exit within 15 seconds; refusing to force-kill it",identity.pid);}util::clear_service_identity(&settings.security.data_dir,identity)?;println!("Stopped EndlessVibe PID {}",identity.pid);Ok(true)
}
fn validate_process_control(c:&Cli)->Result<()> {
    let count=[c.status,c.stop,c.restart].into_iter().filter(|v|*v).count();if count>1{bail!("Use only one of --status, --stop or --restart");}
    if count>0&&(c.init||!c.workspaces.is_empty()||!c.add_projects.is_empty()||c.public_url.is_some()||c.bind.is_some()||c.read_only||c.no_exec||c.allow_shell||c.unsafe_host||c.check_sandbox||c.issue_token||c.revoke_all||c.rotate_key||c.audit){bail!("--status/--stop/--restart may only be combined with --config");}Ok(())
}
fn workspace_arg(value:&str)->Result<WorkspaceConfig>{
    let (id,raw)=value.split_once('=').context("Workspace format is ID=/absolute/root/path")?;
    if !config::valid_id(id){bail!("Workspace ID must contain only ASCII letters, digits, '_' or '-' and be at most 64 characters");}
    let p=PathBuf::from(raw);if !p.is_absolute(){bail!("Workspace paths must be absolute");}let p=p.canonicalize().with_context(||format!("Cannot resolve workspace root {}",p.display()))?;
    Ok(WorkspaceConfig{id:id.into(),path:p,projects:vec![],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None})
}
fn project_arg(cfg:&Config,value:&str,read_only:bool,no_exec:bool)->Result<(usize,ProjectConfig)>{
    let (selector,raw)=value.split_once('=').context("Project format is WORKSPACE[:PROJECT]=/absolute/project/path")?;
    let (workspace_id,explicit_id)=match selector.split_once(':'){Some((w,p))=>(w,Some(p)),None=>(selector,None)};
    if !config::valid_id(workspace_id){bail!("Invalid workspace ID");}
    let index=cfg.workspaces.iter().position(|w|w.id==workspace_id).with_context(||format!("Workspace {workspace_id} is not configured"))?;
    let root=cfg.workspaces[index].path.canonicalize().with_context(||format!("Cannot resolve workspace root {}",cfg.workspaces[index].path.display()))?;
    let path=PathBuf::from(raw);if !path.is_absolute(){bail!("Project paths must be absolute");}let absolute=path.canonicalize().with_context(||format!("Cannot resolve project path {}",path.display()))?;
    if absolute==root||!absolute.starts_with(&root){bail!("Project must be a strict descendant of workspace {workspace_id} ({})",root.display());}
    let relative=absolute.strip_prefix(&root)?.to_owned();
    let inferred=absolute.file_name().and_then(|v|v.to_str()).filter(|v|!v.is_empty()).context("Cannot infer project ID; use WORKSPACE:PROJECT=/absolute/path")?;
    let id=explicit_id.filter(|v|!v.is_empty()).unwrap_or(inferred);
    if !config::valid_id(id){bail!("Project ID must contain only ASCII letters, digits, '_' or '-' and be at most 64 characters");}
    if cfg.workspaces[index].projects.iter().any(|p|p.id==id){bail!("Project ID {workspace_id}/{id} already exists");}
    for existing in &cfg.workspaces[index].projects{let other=root.join(&existing.path).canonicalize().with_context(||format!("Cannot resolve configured project {workspace_id}/{}",existing.id))?;if absolute==other||absolute.starts_with(&other)||other.starts_with(&absolute){bail!("Project paths inside one workspace must not overlap");}}
    Ok((index,ProjectConfig{id:id.into(),path:relative,allow_write:!read_only,allow_exec:!read_only&&!no_exec,allow_git_commit:!read_only,allow_git_mutation:false,allow_git_push:false}))
}
fn workspace_table_offsets(text:&str)->Vec<usize>{let mut out=Vec::new();let mut offset=0;for line in text.split_inclusive('\n'){if line.trim()=="[[workspaces]]"{out.push(offset);}offset+=line.len();}out}
fn insert_project_text(text:&mut String,workspace_index:usize,project:&ProjectConfig)->Result<()>{let offsets=workspace_table_offsets(text);let insert=if workspace_index+1<offsets.len(){offsets[workspace_index+1]}else{text.len()};let mut block=String::new();if insert>0&&!text[..insert].ends_with('\n'){block.push('\n');}block.push_str("\n[[workspaces.projects]]\n");block.push_str(&toml::to_string(project)?);text.insert_str(insert,&block);Ok(())}
fn replace_config(path:&Path,bytes:&[u8])->Result<()>{
    let md=std::fs::symlink_metadata(path).with_context(||format!("Read metadata for {}",path.display()))?;if !md.is_file()||md.file_type().is_symlink()||md.nlink()!=1||md.uid()!=unsafe{libc::geteuid()}{bail!("Config must be a regular, single-link file owned by the service user");}
    let parent=path.parent().context("Config needs a parent directory")?;let name=path.file_name().and_then(|v|v.to_str()).unwrap_or("config.toml");let temp=parent.join(format!(".{name}.{}.tmp",util::random_secret()?));
    util::private_create(&temp,bytes)?;if let Err(e)=std::fs::rename(&temp,path){let _=std::fs::remove_file(&temp);return Err(e).with_context(||format!("Replace {}",path.display()));}Ok(())
}
fn add_projects(c:&Cli,path:&Path)->Result<()>{
    if c.add_projects.is_empty(){return Ok(());}if !c.workspaces.is_empty()||c.init||c.allow_shell||c.unsafe_host||c.check_sandbox||c.issue_token||c.revoke_all||c.rotate_key||c.audit||c.public_url.is_some()||c.bind.is_some(){bail!("--add-project may only be combined with --config, --read-only and --no-exec");}
    let mut cfg=Config::load_file(path)?;util::private_dir(&cfg.security.data_dir)?;let _instance=util::single_instance(&cfg.security.data_dir.join("service.lock")).context("Stop the running EndlessVibe service before changing projects")?;
    let mut text=std::fs::read_to_string(path)?;let mut added=Vec::new();
    for value in &c.add_projects{
        let (index,project)=project_arg(&cfg,value,c.read_only,c.no_exec)?;
        let workspace_id=cfg.workspaces[index].id.clone();cfg.workspaces[index].projects.push(project.clone());
        cfg.validate()?;let checked=workspace::load(&cfg,path)?;drop(checked);insert_project_text(&mut text,index,&project)?;
        added.push((workspace_id,project));
    }
    let verify:Config=toml::from_str(&text).context("Generated project config is invalid")?;verify.validate()?;let checked=workspace::load(&verify,path)?;drop(checked);replace_config(path,text.as_bytes())?;
    for (workspace,project) in &added{eprintln!("Added project {}/{} = {} (write={}, exec={}, commit={})",workspace,project.id,project.path.display(),project.allow_write,project.allow_exec,project.allow_git_commit);}eprintln!("Restart EndlessVibe to load the updated project list.");Ok(())
}
fn initialize(c:&Cli,path:&std::path::Path)->Result<()>{
    if path.exists(){bail!("{} already exists; edit it instead of overwriting",path.display());}
    if c.workspaces.is_empty(){bail!("--init requires at least one explicit --workspace ID=/absolute/root/path");}
    if c.read_only||c.no_exec{bail!("--read-only/--no-exec apply to --add-project, not workspace roots");}
    let mut cfg=Config::default();if let Some(url)=&c.public_url{cfg.server.public_url=url.trim_end_matches('/').into();}if let Some(bind)=&c.bind{cfg.server.bind=bind.parse()?;}
    if c.unsafe_host{cfg.execution.backend="host".into();cfg.execution.acknowledge_unsafe_host_execution=true;}cfg.execution.allow_shell=c.allow_shell;
    for value in &c.workspaces{cfg.workspaces.push(workspace_arg(value)?);}
    cfg.validate()?;util::private_dir(path.parent().context("Config needs a parent directory")?)?;util::private_dir(&cfg.security.data_dir)?;
    let key_path=cfg.security.data_dir.join("owner.key");if !key_path.exists(){util::private_create(&key_path,format!("{}\n",util::random_secret()?).as_bytes())?;}
    // Root probing validates kernel support, credentials, overlaps and locks before publishing config.
    let probe=Runtime::new(cfg.clone(),path)?;drop(probe);
    util::private_create(path,toml::to_string_pretty(&cfg)?.as_bytes())?;
    eprintln!("Created {}\nOwner key: {} (keep private; enter only in your own OAuth browser page)\nStart with: endlessvibe --config {}\nChatGPT URL: {}/mcp (OAuth)\n",path.display(),key_path.display(),path.display(),cfg.server.public_url);Ok(())
}
#[tokio::main]
async fn main()->Result<()>{
    // Credentials, jobs and backups must not become group/world-readable, including SQLite sidecars.
    unsafe{libc::umask(0o077);}
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_|"endlessvibe=info,rmcp=warn".into())).init();
    let Some(cli)=arguments()? else{return Ok(());};if unsafe{libc::geteuid()}==0{bail!("Run EndlessVibe as a non-root user; root service execution is refused");}validate_process_control(&cli)?;let path=cli.config.clone().unwrap_or_else(config::default_config_path);let path=if path.is_absolute(){path}else{std::env::current_dir()?.join(path)};
    if cli.status||cli.stop||cli.restart{let settings=Config::load(&path)?;if cli.status{match service_state(&settings)?{ServiceState::Stopped=>println!("stopped"),ServiceState::Running(identity)=>println!("running pid={} start_ticks={}",identity.pid,identity.start_ticks),ServiceState::LegacyRunning=>println!("running legacy_instance=true pid_file=false")};return Ok(());}if cli.stop{return stop_service(&settings).await.map(|_|());}if cli.restart{let _=stop_service(&settings).await?;println!("Starting EndlessVibe with {}",path.display());}}
    if cli.init{if !cli.add_projects.is_empty(){bail!("Use --workspace with --init; --add-project modifies an existing config");}if cli.check_sandbox{bail!("--check-sandbox probes an existing config; do not combine it with --init");}return initialize(&cli,&path);}
    if !cli.add_projects.is_empty(){return add_projects(&cli,&path);}
    if !cli.workspaces.is_empty()||cli.read_only||cli.no_exec||cli.allow_shell||cli.unsafe_host{bail!("Workspace/permission flags require --init or --add-project");}
    let mut settings=Config::load(&path)?;if let Some(url)=cli.public_url{settings.server.public_url=url.trim_end_matches('/').into();}if let Some(bind)=cli.bind{settings.server.bind=bind.parse()?;}settings.validate()?;
    if cli.check_sandbox{if cli.issue_token||cli.revoke_all||cli.rotate_key||cli.audit{bail!("--check-sandbox cannot be combined with credential/audit maintenance operations");}let programs=process::probe_bubblewrap(&settings).await?;for(program,path)in &programs{println!("sandbox program: {program} -> {path}");}println!("bubblewrap sandbox probe: ok ({} allowed/required programs visible)",programs.len());return Ok(());}
    if cli.issue_token||cli.revoke_all||cli.rotate_key||cli.audit{
        util::private_dir(&settings.security.data_dir)?;let settings=Arc::new(settings);let db=Arc::new(Store::open(&settings.security.data_dir.join("state.sqlite3"))?);let auth=Auth::new(settings.clone(),db.clone())?;
        if cli.rotate_key{let _lock=util::single_instance(&settings.security.data_dir.join("service.lock"))?;auth.revoke_all()?;let temp=settings.security.data_dir.join(format!("owner-{}.tmp",util::random_secret()?));util::private_create(&temp,format!("{}\n",util::random_secret()?).as_bytes())?;std::fs::rename(temp,settings.security.data_dir.join("owner.key"))?;eprintln!("Owner key rotated; all prior tokens revoked. Restart the service and relink OAuth.");}
        if cli.revoke_all{auth.revoke_all()?;eprintln!("All access/refresh tokens and pending authorizations revoked; OAuth client IDs retained.");}
        if cli.issue_token{println!("{}",auth.issue_local_token()?);}
        if cli.audit{println!("{}",serde_json::to_string_pretty(&db.audits(100)?)?);}
        return Ok(());
    }
    let listener=tokio::net::TcpListener::bind(settings.server.bind).await.with_context(||format!("Cannot listen on {}; stop the previous EndlessVibe process",settings.server.bind))?;settings.server.bind=listener.local_addr()?;
    let rt=Runtime::new(settings,&path)?;let _service_pid=util::install_service_identity(&rt.config.security.data_dir)?;let app=server::create_router(rt.clone());
    let telemetry_rt=rt.clone();let telemetry_task=tokio::spawn(async move{let mut interval=tokio::time::interval(std::time::Duration::from_secs(1));interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);loop{tokio::select!{_=telemetry_rt.shutdown.cancelled()=>break,_=interval.tick()=>{let(requests,rx_bytes,tx_bytes)=telemetry_rt.take_http_traffic();if requests>0||rx_bytes>0||tx_bytes>0{if let Err(error)=telemetry_rt.db.record_traffic(requests,rx_bytes,tx_bytes){tracing::warn!(error=%error,"Could not persist HTTP traffic metrics");}telemetry_rt.publish_dashboard("metrics_changed",serde_json::json!({"http_requests":requests,"rx_bytes":rx_bytes,"tx_bytes":tx_bytes,"active_jobs":telemetry_rt.jobs.active_count()}));}}}}let(requests,rx_bytes,tx_bytes)=telemetry_rt.take_http_traffic();if requests>0||rx_bytes>0||tx_bytes>0{let _=telemetry_rt.db.record_traffic(requests,rx_bytes,tx_bytes);}});
    let dashboard_listener=tokio::net::TcpListener::bind("127.0.0.1:20001")
        .await
        .context("Cannot listen on Dashboard 127.0.0.1:20001")?;
    let dashboard_app=server::create_dashboard_router(rt.clone());
    let dashboard_task=tokio::spawn(async move{
        if let Err(error)=axum::serve(dashboard_listener,dashboard_app).await{
            tracing::error!(error=%error,"Dashboard server failed");
        }
    });

    let(workspace_count,project_count)=rt.workspace_project_counts();tracing::info!(version=env!("CARGO_PKG_VERSION"),listen=%rt.config.server.bind,workspaces=workspace_count,projects=project_count,backend=%rt.config.execution.backend,"EndlessVibe started; private MCP endpoints require OAuth");
    if rt.config.execution.backend=="host"{tracing::warn!("UNSANDBOXED host execution was explicitly enabled; tools have the service user's permissions");}
    if rt.config.execution.backend=="bubblewrap"&&!rt.config.execution.bubblewrap.exists(){tracing::warn!("bubblewrap is not installed; file/Git tools work, command jobs fail closed until it is installed");}
    eprintln!("Dashboard: http://127.0.0.1:20001/\nMCP: {}/mcp\nAuthentication: OAuth (authorization code + S256 PKCE)\n",rt.config.server.public_url);
    let shutdown_rt=rt.clone();let result=axum::serve(listener,app).with_graceful_shutdown(async move{shutdown_signal().await;shutdown_rt.jobs.cancel_all();shutdown_rt.shutdown.cancel();}).await;
    rt.shutdown.cancel();dashboard_task.abort();let _=dashboard_task.await;let _=telemetry_task.await;
    rt.jobs.shutdown().await;rt.wait_for_operations().await;result.context("HTTP server failed")
}
async fn shutdown_signal(){
    let ctrl=async{if let Err(e)=tokio::signal::ctrl_c().await{tracing::error!(error=%e,"Cannot install Ctrl-C handler");}};
    let term=async{match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()){Ok(mut signal)=>{signal.recv().await;},Err(e)=>{tracing::error!(error=%e,"Cannot install SIGTERM handler");std::future::pending::<()>().await;}}};
    tokio::select!{_=ctrl=>{},_=term=>{}};
}

#[cfg(test)]mod tests{
    use super::*;
    fn cfg(root:&Path,state:&Path)->Config{let mut c=Config::default();c.security.data_dir=state.into();c.workspaces=vec![WorkspaceConfig{id:"root".into(),path:root.into(),projects:vec![],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None}];c}
    #[test]fn workspace_parser_creates_root_only(){let t=tempfile::tempdir().unwrap();let w=workspace_arg(&format!("root={}",t.path().display())).unwrap();assert_eq!(w.id,"root");assert!(w.projects.is_empty());assert!(w.allow_write.is_none()&&w.allow_exec.is_none()&&w.allow_git_commit.is_none()&&w.allow_git_mutation.is_none());}
    #[test]fn project_parser_infers_id_and_permissions(){let t=tempfile::tempdir().unwrap();let root=t.path().join("root");let child=root.join("demo");std::fs::create_dir_all(&child).unwrap();let c=cfg(&root,&t.path().join("state"));let (_,p)=project_arg(&c,&format!("root={}",child.display()),false,false).unwrap();assert_eq!(p.id,"demo");assert_eq!(p.path,PathBuf::from("demo"));assert!(p.allow_write&&p.allow_exec&&p.allow_git_commit&&!p.allow_git_mutation&&!p.allow_git_push);}
    #[test]fn project_parser_rejects_outside_root(){let t=tempfile::tempdir().unwrap();let root=t.path().join("root");let outside=t.path().join("outside");std::fs::create_dir_all(&root).unwrap();std::fs::create_dir_all(&outside).unwrap();let c=cfg(&root,&t.path().join("state"));assert!(project_arg(&c,&format!("root={}",outside.display()),false,false).is_err());}
    #[test]fn add_project_preserves_config_comments(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let root=t.path().join("projects");let child=root.join("demo");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(&child).unwrap();let c=cfg(&root,&state);let path=t.path().join("config.toml");let mut text=toml::to_string_pretty(&c).unwrap();text.push_str("\n# keep-this-comment\n");std::fs::write(&path,text).unwrap();let mut cli=Cli::default();cli.add_projects.push(format!("root={}",child.display()));add_projects(&cli,&path).unwrap();let after=std::fs::read_to_string(&path).unwrap();assert!(after.contains("# keep-this-comment"));let loaded=Config::load_file(&path).unwrap();assert_eq!(loaded.workspaces.len(),1);assert_eq!(loaded.workspaces[0].projects[0].id,"demo");assert_eq!(loaded.workspaces[0].projects[0].path,PathBuf::from("demo"));}
}
