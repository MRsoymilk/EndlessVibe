//! Policy and lifecycle for the Windows AppContainer command backend.
//!
//! Planning does not grant access or launch code. `prepare` runs only while
//! Jobs holds the project lock, command slot, and service-update/cache guard.
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{collections::{BTreeMap,HashSet},ffi::OsString,path::{Path,PathBuf},sync::Arc,time::{Duration,Instant}};
use tokio_util::sync::CancellationToken;
use crate::{config::Config,security::paths::Root,workspace::{Project,Workspace}};
use super::{windows_acl::{self,Access,Grant,Lease,PrivateToolsSeal},windows_command::{self,CommandSpec,Identity,Outcome,RunningCommand},windows_execution_recovery::VerifiedRecovery,windows_toolchains::{self,Toolchains},windows_tool_copy::check_deadline};

pub(super) fn validate_environment(environment:&BTreeMap<String,String>)->Result<()>{
    let mut names=HashSet::new();
    for name in environment.keys(){
        let normalized=name.to_ascii_uppercase();
        if !names.insert(normalized.clone()){
            bail!("Windows environment contains duplicate case-insensitive variable {name}");
        }
        if ["HOME","PATH","CARGO_HOME","RUSTUP_HOME","XDG_CACHE_HOME","LANG","LC_ALL","TERM",
            "ENDLESSVIBE_JOB_SUMMARY","USERPROFILE","APPDATA","LOCALAPPDATA","SYSTEMROOT",
            "WINDIR","SYSTEMDRIVE","TEMP","TMP","HOMEDRIVE","HOMEPATH","COMSPEC"]
            .contains(&normalized.as_str()){
            bail!("Windows AppContainer environment variable {name} is managed by EndlessVibe");
        }
    }
    Ok(())
}

/// An immutable authorized command plan; constructing it has no ACL side effects.
pub struct Plan{
    project:PathBuf,
    cwd:PathBuf,
    executable:PathBuf,
    args:Vec<OsString>,
    toolchains:Toolchains,
    needs_cargo_cache:bool,
    environment:BTreeMap<String,String>,
    state_dir:PathBuf,
    git_author_name:String,
    git_author_email:String,
    allow_git_mutation:bool,
    memory_limit_mb:u64,
    max_processes:u64,
}
impl Plan{
    pub(super) fn new(config:&Config,project:&Project,program:&str,args:&[String],cwd:PathBuf,
        shell:bool,project_programs:&[String],dependency_programs:&[String],
        environment:&BTreeMap<String,String>)->Result<Self>{
        validate_environment(environment)?;
        if config.execution.backend!="appcontainer"{bail!("Expected the AppContainer execution backend");}
        let actual=if shell{"powershell"}else{program};
        // Project manifests expose optional tools, but only this command's
        // dependencies justify reading its lockfile and host package cache.
        let needs_cargo_cache=actual=="cargo"||dependency_programs.iter().any(|name|name=="cargo");
        let mut programs=project_programs.to_vec();
        programs.extend(dependency_programs.iter().cloned());
        programs.extend(config.execution.required_programs.iter().cloned());
        programs.push(actual.to_owned());
        if actual=="cargo"{programs.push("rustc".into());}
        programs.sort();programs.dedup();
        let toolchains=windows_toolchains::discover(config,&programs)?;
        let executable=toolchains.programs.get(actual).cloned()
            .with_context(||format!("Configured native executable {actual} is unavailable"))?;
        Ok(Self{project:project.root.path.clone(),cwd,executable,
            args:args.iter().map(OsString::from).collect(),toolchains,needs_cargo_cache,
            environment:environment.clone(),
            state_dir:config.security.data_dir.clone(),git_author_name:config.git.author_name.clone(),
            git_author_email:config.git.author_email.clone(),allow_git_mutation:project.config.allow_git_mutation,
            memory_limit_mb:config.execution.memory_limit_mb,max_processes:config.execution.max_processes})
    }

    pub(super) fn prepare(mut self,config_path:&Path)->Result<Prepared>{
        // Preparation precedes the command's own runtime timeout. Bound it
        // separately, and never launch after expensive copy/ACL preparation
        // has consumed this deadline.
        let deadline=Instant::now()+Duration::from_secs(90);
        let identity=Identity::create(&self.state_dir)?;
        let summary_path=identity.directory().join("job-summary.json");
        // Own the identity before the first fallible preparation step. Drop is
        // only a last resort; every ordinary error explicitly verifies cleanup.
        let mut prepared=Prepared{command:None,lease:None,profile:None,tool_seal:None,
            identity,summary_path,finished:false};
        let result=(||->Result<()>{
            check_deadline(deadline)?;
            // Keep the original profile root pinned until summary capture.
            prepared.profile=Some(Root::open(prepared.identity.directory())?);
            let mut environment=windows_command::base_environment(&prepared.identity)?.into_iter()
                .map(|(key,value)|(OsString::from(key.to_string_lossy().to_ascii_uppercase()),value)).collect::<BTreeMap<_,_>>();
            let cargo_home=prepared.identity.directory().join("cargo");
            if self.needs_cargo_cache{
                self.toolchains.prepare_cargo_home_for_project(&cargo_home,&self.project)?;
            }else{
                self.toolchains.prepare_cargo_home(&cargo_home)?;
            }
            check_deadline(deadline)?;
            let cache=prepared.identity.directory().join("cache");
            std::fs::create_dir_all(&cache)?;
            let private_tools=prepared.identity.directory().join("tools");
            let staged=self.toolchains.prepare_readonly_copies(&private_tools,deadline)?;
            // The entry command was captured before staging. Remap it too,
            // including direct MSVC jobs whose executable needed a copy.
            self.executable=windows_toolchains::remap_path(&self.executable,&staged.mappings);
            if !staged.mappings.is_empty(){
                prepared.tool_seal=Some(windows_acl::seal_private_tools(
                    prepared.identity.directory(),prepared.identity.sid_string())?);
            }
            // The seal's roots remain pinned in Prepared; all file-writing
            // handles are closed and effective read-only access is verified.
            drop(staged);
            check_deadline(deadline)?;
            let system=crate::tools::windows_trusted_worker::system32_directory()?;
            let mut paths=self.toolchains.path_entries.clone();
            paths.push(system);
            paths.dedup();
            environment.extend(self.toolchains.environment.into_iter()
                .map(|(key,value)|(OsString::from(key.to_string_lossy().to_ascii_uppercase()),value)));
            environment.insert("PATH".into(),std::env::join_paths(paths).context("Build native sandbox PATH")?);
            for (key,value) in [("CARGO_HOME",cargo_home.as_os_str()),("XDG_CACHE_HOME",cache.as_os_str()),
                ("ENDLESSVIBE_JOB_SUMMARY",prepared.summary_path.as_os_str())]{environment.insert(key.into(),value.to_owned());}
            for (key,value) in [("LANG","C.UTF-8"),("LC_ALL","C.UTF-8"),("TERM","dumb"),
                ("CARGO_NET_OFFLINE","true"),("GIT_CONFIG_NOSYSTEM","1"),("GIT_TERMINAL_PROMPT","0"),
                ("GIT_MERGE_AUTOEDIT","no")]{environment.insert(key.into(),value.into());}
            for (name,value) in self.environment{environment.insert(name.to_ascii_uppercase().into(),value.into());}
            for key in ["GIT_AUTHOR_NAME","GIT_COMMITTER_NAME"]{environment.insert(key.into(),self.git_author_name.clone().into());}
            for key in ["GIT_AUTHOR_EMAIL","GIT_COMMITTER_EMAIL"]{environment.insert(key.into(),self.git_author_email.clone().into());}
            let mut grants=vec![Grant{path:self.project.clone(),access:Access::ReadWrite}];
            grants.extend(self.toolchains.readonly_roots.into_iter().map(|path|Grant{path,access:Access::ReadExecute}));
            let git=self.project.join(".git");
            if !self.allow_git_mutation{
                match std::fs::symlink_metadata(&git){
                    Ok(_)=>grants.push(Grant{path:git,access:Access::ReadExecute}),
                    Err(error)if error.kind()==std::io::ErrorKind::NotFound=>{},
                    Err(error)=>return Err(error).context("Inspect Git metadata before granting project access"),
                }
            }
            let protected=vec![self.state_dir.clone(),config_path.canonicalize()
                .unwrap_or_else(|_|config_path.to_owned())];
            check_deadline(deadline)?;
            let lease=match Lease::acquire(&acl_journal_dir(&self.state_dir),prepared.identity.sid_string(),&grants,&protected){
                Ok(lease)=>lease,
                Err(error)=>{
                    if crate::error::code(&error)=="SANDBOX_CLEANUP_PENDING"{prepared.identity.defer_cleanup();}
                    return Err(error);
                }
            };
            prepared.lease=Some(lease);
            check_deadline(deadline)?;
            let spec=CommandSpec{program:self.executable,args:self.args,cwd:self.cwd,environment,
                memory_limit_mb:self.memory_limit_mb,max_processes:self.max_processes};
            prepared.command=Some(windows_command::spawn(&prepared.identity,&spec)?);
            Ok(())
        })();
        if let Err(error)=result{
            if let Err(cleanup)=prepared.finish(){
                return Err(crate::error::coded("SANDBOX_CLEANUP_PENDING",false,
                    format!("Windows sandbox preparation failed ({error:#}); cleanup requires service recovery ({cleanup:#})")));
            }
            return Err(error);
        }
        Ok(prepared)
    }
}

pub(super) fn acl_journal_dir(state_dir:&Path)->PathBuf{state_dir.join("appcontainer-acl")}

pub(super) fn diagnostics(config:&Config,programs:&[String])->Result<Value>{
    let isolated=config.execution.backend=="appcontainer";
    let mut discovery=config.clone();
    if !isolated{discovery.execution.readonly_mounts.clear();}
    let tools=windows_toolchains::discover(&discovery,programs)?;
    let rows=programs.iter().map(|program|{
        let path=tools.programs.get(program);
        json!({"program":program,"host_path":path,"sandbox_path":if isolated{path}else{None},
            "status":if path.is_none(){"missing"}else if isolated{"available"}else{"host-only"},
            "reason":if path.is_none(){Some("configured_native_program_missing")}else{None},
            "allowed":config.execution.allowed_programs.contains(program),
            "required":config.execution.required_programs.contains(program),
            "install_hint":if path.is_none(){Some("Install the native Windows tool and add its exact executable directory to execution.path")}else{None},
            "suggestion":Value::Null})
    }).collect::<Vec<_>>();
    let mounts=tools.readonly_roots.iter().map(|path|json!({"source":path,"target":path,
        "source_exists":path.exists(),"target_allowed":true,
        "automatic":!config.execution.readonly_mounts.iter().any(|mount|mount.source==*path)})).collect::<Vec<_>>();
    Ok(json!({"backend":config.execution.backend,"host_os":"windows",
        "path":std::env::join_paths(&tools.path_entries)?.to_string_lossy(),
        "configured_path":config.execution.path,"auto_discover_toolchains":config.execution.auto_discover_toolchains,
        "namespace_ok":false,"programs":rows,"readonly_mounts":mounts,
        "network":if isolated{false}else{config.execution.backend=="host"||config.execution.allow_network},"shell":config.execution.allow_shell,
        "command_execution_tested":false,"acl_access_verified":"at_each_job_preparation",
        "suggested_config":{"prepend_path_entries":[],"readonly_mounts":[],"toml":""}}))
}

pub(super) struct Prepared{
    command:Option<RunningCommand>,
    lease:Option<Lease>,
    profile:Option<Root>,
    tool_seal:Option<PrivateToolsSeal>,
    identity:Identity,
    summary_path:PathBuf,
    finished:bool,
}
impl Prepared{
    pub(super) async fn run(&mut self,cancel:&CancellationToken,timeout:Duration,
        output:impl FnMut(&'static str,&[u8])->Result<()>)->Result<Outcome>{
        self.command.as_mut().context("AppContainer process was already closed")?
            .run(cancel,timeout,output).await
    }
    fn close_process(&mut self)->Result<()>{
        drop(self.command.take());
        if !self.identity.job_empty(){
            if let Some(lease)=&mut self.lease{lease.defer_cleanup();}
            self.identity.defer_cleanup();
            return Err(crate::error::coded("SANDBOX_CLEANUP_PENDING",false,
                "Windows did not confirm that the sandbox process tree exited; project access remains locked until service recovery"));
        }
        Ok(())
    }
    pub(super) fn capture_summary(&mut self)->Result<(Value,Value)>{
        self.close_process()?;
        let profile=self.profile.as_ref().context("AppContainer profile is already closed")?;
        if let Err(error)=profile.unchanged_root(){
            return Ok((Value::Null,json!({"status":"error","reported":true,"cleanup_complete":false,
                "error":crate::util::bounded_text(&format!("Unsafe sandbox summary root: {error}"),512)})));
        }
        Ok(super::jobs::capture_job_summary(&self.summary_path))
    }
    pub(super) fn finish(&mut self)->Result<()>{
        if self.finished{return Ok(());}
        self.close_process()?;
        if let Some(lease)=&mut self.lease{
            if let Err(error)=lease.finish(){
                lease.defer_cleanup();self.identity.defer_cleanup();
                return Err(crate::error::coded("SANDBOX_CLEANUP_PENDING",false,
                    format!("Windows sandbox ACL cleanup requires service recovery: {error}")));
            }
        }
        drop(self.lease.take());
        // DeleteAppContainerProfile cannot remove a profile while its pinned
        // summary root or child file handles remain open.
        drop(self.tool_seal.take());
        drop(self.profile.take());
        self.identity.finish().map_err(|error|crate::error::coded("SANDBOX_CLEANUP_PENDING",false,
            format!("Windows sandbox profile cleanup requires service recovery: {error}")))?;
        self.finished=true;
        Ok(())
    }
}
impl Drop for Prepared{
    fn drop(&mut self){if !self.finished{if let Err(error)=self.finish(){
        tracing::error!(error=%error,"Windows sandbox cleanup deferred; recovery journals retained");
    }}}
}

/// Recovery is allowed only under the service instance lock and before any
/// new AppContainer profile or job can be created.
pub(crate) fn recover_pending(config:&Config,workspaces:&BTreeMap<String,Arc<Workspace>>,
    proofs:&VerifiedRecovery)->Result<usize>{
    let journal=acl_journal_dir(&config.security.data_dir);
    let pending=windows_acl::pending_sids(&journal)?;
    proofs.require_acl_sids(&pending)?;
    if pending.is_empty(){return Ok(0);}
    let mut roots=workspaces.values().map(|workspace|workspace.root.path.clone()).collect::<Vec<_>>();
    for mount in &config.execution.readonly_mounts{roots.push(mount.source.canonicalize()?);}
    // A dynamic auxiliary executable must come from configured PATH. These
    // directories authorize only removal of a protected journal's exact SID,
    // never new grants; recover_pending also pins and checks each grant's ID.
    for path in std::env::split_paths(&config.execution.path){
        if let Ok(path)=path.canonicalize(){roots.push(path);}
    }
    let mut programs=config.execution.allowed_programs.clone();
    programs.extend(config.execution.required_programs.iter().cloned());
    for project in workspaces.values().flat_map(|workspace|workspace.projects.values()){
        programs.extend(super::process::detect_project_programs(project));
    }
    if config.execution.allow_shell{programs.push("powershell".into());}
    // Only these native tool kinds derive roots outside their configured bin
    // directory (Rustup/sysroot and MSVC/SDK). Resolve them by the same bounded
    // discovery rules as a command; never allow a whole home/install ancestor.
    programs.extend(["cargo","rustc","rustdoc","rustfmt","cargo-fmt","cargo-clippy",
        "clippy-driver","cmake","ninja","cl","link","lib"].map(str::to_owned));
    programs.sort();programs.dedup();
    // Explicit roots are already present above. They need not be interpreted
    // as active sandbox mounts when recovering after a backend change.
    let mut discovery=config.clone();
    discovery.execution.readonly_mounts.clear();
    match windows_toolchains::discover(&discovery,&programs){
        Ok(toolchains)=>roots.extend(toolchains.readonly_roots),
        Err(error)=>{
            tracing::warn!(error=%error,"Resolve optional recovery tools individually; every recorded root must still be authorized");
            for program in programs{
                match windows_toolchains::discover(&discovery,std::slice::from_ref(&program)){
                    Ok(toolchains)=>roots.extend(toolchains.readonly_roots),
                    Err(error)=>tracing::warn!(program=%program,error=%error,
                        "Recovery tool is unavailable; no derived root was authorized"),
                }
            }
        }
    }
    roots.sort();roots.dedup();
    windows_acl::recover_pending(&journal,&roots)
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn windows_environment_rejects_case_aliases_and_private_paths(){
        for name in ["Path","UserProfile","SystemRoot","tmp","Cargo_Home","ComSpec"]{
            let env=BTreeMap::from([(name.to_owned(),"outside".to_owned())]);
            assert!(validate_environment(&env).is_err(),"{name}");
        }
        assert!(validate_environment(&BTreeMap::from([
            ("PROJECT_VALUE".into(),"one".into()),("project_value".into(),"two".into())])).is_err());
        assert!(validate_environment(&BTreeMap::from([("PROJECT_VALUE".into(),"ok".into())])).is_ok());
    }
    #[test]
    fn inactive_diagnostics_accept_legacy_mounts_without_changing_configuration(){
        let temporary=tempfile::tempdir().unwrap();
        let bin=temporary.path().join("bin");
        let extra=temporary.path().join("extra");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        // Diagnostics inspect executable paths without running fixture bytes.
        std::fs::write(bin.join("ev_probe.exe"),b"native discovery fixture").unwrap();
        let mut config=Config::default();
        config.execution.path=std::env::join_paths([&bin]).unwrap().into_string().unwrap();
        config.execution.allowed_programs=vec!["ev_probe".into()];
        config.execution.auto_discover_toolchains=false;
        config.execution.readonly_mounts=vec![crate::config::ReadOnlyMount{
            source:extra.clone(),target:PathBuf::from("/opt/legacy-tool")}];
        for backend in ["host","disabled"]{
            config.execution.backend=backend.into();
            config.execution.acknowledge_unsafe_host_execution=backend=="host";
            config.validate().unwrap();
            let value=diagnostics(&config,&["ev_probe".into()]).unwrap();
            assert_eq!(value["programs"][0]["status"],"host-only");
            assert_eq!(value["command_execution_tested"],false);
            assert_eq!(config.execution.readonly_mounts[0].source,extra);
            assert_eq!(config.execution.readonly_mounts[0].target,PathBuf::from("/opt/legacy-tool"));
        }
        config.execution.backend="appcontainer".into();
        config.execution.acknowledge_unsafe_host_execution=false;
        assert!(diagnostics(&config,&["ev_probe".into()]).is_err());
    }
    #[test]
    fn recovery_requires_execution_proof_before_revoking_an_authorized_path(){
        let temporary=tempfile::tempdir().unwrap();
        let project=temporary.path().join("project");
        let state=temporary.path().join("state");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let mut config=Config::default();
        config.security.data_dir=state.clone();
        // This path is a valid revoke-only boundary. Without the proof gate,
        // ACL recovery would actually remove this controlled fixture lease.
        config.execution.path=std::env::join_paths([&project]).unwrap().into_string().unwrap();
        config.execution.allowed_programs.clear();
        config.execution.auto_discover_toolchains=false;
        let journal=acl_journal_dir(&state);
        let sid="S-1-15-2-31-41-59-26-53-58-9793";
        let mut lease=Lease::acquire(&journal,sid,
            &[Grant{path:project,access:Access::ReadWrite}],&[state.clone()]).unwrap();
        let proofs=super::super::windows_execution_recovery::verify_recovery(&state).unwrap();
        let error=recover_pending(&config,&BTreeMap::new(),&proofs).unwrap_err();
        assert_eq!(crate::error::code(&error),"SANDBOX_CLEANUP_PENDING");
        assert_eq!(windows_acl::pending_sids(&journal).unwrap(),
            std::collections::BTreeSet::from([sid.to_owned()]));
        // The fixture never launched a process; explicitly close its lease.
        lease.finish().unwrap();
        assert!(windows_acl::pending_sids(&journal).unwrap().is_empty());
        assert_eq!(proofs.finish().unwrap(),0);
    }
}
