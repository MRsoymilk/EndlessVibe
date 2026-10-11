// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: windows_service_deploy.rs
// Description: Opt-in, hash-bound Windows EndlessVibe service self-update.
// This never invokes PowerShell, Shell, arbitrary binaries or user commands.

use anyhow::{bail,Context,Result};
use serde::{Serialize,Deserialize};
use serde_json::{json,Value};
use fs4::fs_std::FileExt;
use std::{
    fs::{self,File,OpenOptions},
    io::{Read,Write},
    os::windows::{ffi::OsStrExt,fs::OpenOptionsExt,process::CommandExt},
    path::{Path,PathBuf},
    process::{Child,Command,Stdio},
    sync::Arc,
    thread,time::{Duration,Instant},
};
use crate::{config::Config,tools::windows_service_update as preflight,util};

const WINDOWS_DETACHED_PROCESS:u32=0x00000008;
const WINDOWS_NEW_PROCESS_GROUP:u32=0x00000200;
const FILE_FLAG_OPEN_REPARSE_POINT:u32=0x0020_0000;
const EXE_NAME:&str="endlessvibe.exe";
const ACTIVE:&str="active.json";
const MAX_RECORD_BYTES:usize=8192;
const MOVEFILE_WRITE_THROUGH:u32=0x0000_0008;
#[link(name="kernel32")]
unsafe extern "system"{
    fn ReplaceFileW(replaced:*const u16,replacement:*const u16,
        backup:*const u16,flags:u32,excluded:*mut std::ffi::c_void,
        reserved:*mut std::ffi::c_void)->i32;
    fn MoveFileExW(existing:*const u16,destination:*const u16,flags:u32)->i32;
}

#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct Request{
    nonce:String,
    request_id:String,
    workspace:String,
    project:String,
    old_sha256:String,
    new_sha256:String,
    config_path:PathBuf,
    project_root:PathBuf,
    old_pid:i32,
    old_start_ticks:u64,
    created_at_unix:u64,
}
fn valid_id(value:&str)->bool{
    !value.is_empty()&&value.len()<=64&&
    value.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'-'||c==b'_')
}
fn validate_request_fields(request:&Request)->Result<()>{
    if !valid_id(&request.request_id)||!valid_id(&request.nonce)
        ||request.nonce.len()<32||!request.nonce.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b))||
        !crate::config::valid_id(&request.workspace)||!crate::config::valid_id(&request.project)||
        !preflight::digest_ok(&request.old_sha256)||
        !preflight::digest_ok(&request.new_sha256)||
        request.old_sha256==request.new_sha256||
        !request.config_path.is_absolute()||
        !request.project_root.is_absolute()||
        request.old_pid<=1||request.old_start_ticks==0||
        request.created_at_unix==0{
        bail!("Invalid or incomplete Windows service update request");
    }
    Ok(())
}
fn validate_request(request:&Request)->Result<()>{
    validate_request_fields(request)?;
    if util::now().saturating_sub(request.created_at_unix)>300||
        request.created_at_unix>util::now().saturating_add(60){
        bail!("Windows service update authorization has expired or is future-dated");
    }
    Ok(())
}
fn update_dir(data_dir:&Path)->PathBuf{data_dir.join("windows-service-updates")}

#[derive(Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperIdentity{
    request_id:String,
    nonce:String,
    pid:i32,
    start_ticks:u64,
}
impl HelperIdentity{
    fn process(&self)->util::ProcessIdentity{
        util::ProcessIdentity{pid:self.pid,start_ticks:self.start_ticks}
    }
}

fn path_exists(path:&Path)->Result<bool>{
    match fs::symlink_metadata(path){
        Ok(_)=>Ok(true),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(false),
        Err(error)=>Err(error.into()),
    }
}
fn result_path(directory:&Path,request_id:&str)->PathBuf{
    directory.join(format!("result-{request_id}.json"))
}
fn working_path(directory:&Path,request_id:&str)->PathBuf{
    directory.join(format!("working-{request_id}.json"))
}
fn helper_path(directory:&Path,request_id:&str)->PathBuf{
    directory.join(format!("helper-{request_id}.json"))
}

// The destination must not replace any previous transaction or release.
// Every caller places source and destination in the same directory/volume.
fn move_new(source:&Path,destination:&Path)->Result<()>{
    if unsafe{MoveFileExW(wide(source).as_ptr(),wide(destination).as_ptr(),
            MOVEFILE_WRITE_THROUGH)}==0{
        return Err(std::io::Error::last_os_error())
            .context("Publish Windows update record without replacing an existing file");
    }
    Ok(())
}

fn publish_record_with(
    destination:&Path,bytes:&[u8],
    write_and_sync:impl FnOnce(&mut File,&[u8])->Result<()>,
)->Result<()>{
    let directory=destination.parent().context("Update record has no directory")?;
    preflight::real_directory(directory)?;
    let temporary=directory.join(format!("record-{}.tmp",util::random_secret()?));
    // Only clean up a file whose create_new succeeded in this invocation.
    let mut file=OpenOptions::new().write(true).create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(&temporary)
        .context("Create private Windows update record")?;
    let written=write_and_sync(&mut file,bytes);
    drop(file);
    let result=written.and_then(|_|move_new(&temporary,destination));
    if result.is_err(){let _=fs::remove_file(&temporary);}
    result
}
fn publish_record(destination:&Path,bytes:&[u8])->Result<()>{
    publish_record_with(destination,bytes,|file,data|{
        file.write_all(data)?;
        file.sync_all()?;
        Ok(())
    })
}
fn persist_report(directory:&Path,request:&Request,report:&Value)->Result<()>{
    publish_record(&result_path(directory,&request.request_id),&serde_json::to_vec(report)?)
        .context("Terminal update result could not be persisted; recovery records were retained")
}

fn read_request(path:&Path)->Result<Request>{
    let request:Request=serde_json::from_slice(&util::private_read(path,MAX_RECORD_BYTES)?)?;
    // Recovery records may be old; only authorization consumption has a TTL.
    validate_request_fields(&request)?;
    Ok(request)
}
fn pending_requests(directory:&Path)->Result<Vec<Request>>{
    let mut requests=Vec::<Request>::new();
    for entry in fs::read_dir(directory)?{
        let entry=entry?;
        let name=entry.file_name();
        let Some(name)=name.to_str() else{continue;};
        let id=if name==ACTIVE{None}
            else if let Some(id)=name.strip_prefix("working-").and_then(|n|n.strip_suffix(".json")){
                if !valid_id(id){bail!("Malformed Windows update recovery filename");}
                Some(id)
            }else{continue;};
        let request=read_request(&entry.path())?;
        if id.is_some_and(|id|id!=request.request_id){
            bail!("Windows update record does not match its transaction filename");
        }
        if let Some(existing)=requests.iter().find(|item|item.request_id==request.request_id){
            if existing!=&request{bail!("Conflicting Windows update recovery records");}
        }else{
            if requests.len()>=32{bail!("Too many unresolved Windows update records");}
            requests.push(request);
        }
    }
    Ok(requests)
}
fn update_lock_held(directory:&Path)->Result<bool>{
    let path=directory.join("update.lock");
    if !path_exists(&path)?{return Ok(false);}
    let file=OpenOptions::new().read(true).write(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(&path)?;
    if !file.metadata()?.is_file()||!crate::platform::single_link(&file){
        bail!("Windows update lock is not a single-link regular file");
    }
    Ok(!file.try_lock_exclusive()?)
}

// Unlike process_identity_alive, an access/inspection failure is not treated
// as proof that a helper exited. PID reuse is checked against creation time.
fn identity_alive(identity:util::ProcessIdentity)->Result<bool>{
    use std::os::windows::io::{AsRawHandle,FromRawHandle,OwnedHandle};
    use windows_sys::Win32::{
        Foundation::{ERROR_INVALID_PARAMETER,FILETIME,WAIT_OBJECT_0,WAIT_TIMEOUT},
        System::Threading::{GetProcessTimes,OpenProcess,WaitForSingleObject,
            PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_SYNCHRONIZE},
    };
    if identity.pid<=1||identity.start_ticks==0{bail!("Invalid update process identity");}
    let raw=unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_SYNCHRONIZE,
        0,identity.pid as u32)};
    if raw.is_null(){
        let error=std::io::Error::last_os_error();
        if error.raw_os_error()==Some(ERROR_INVALID_PARAMETER as i32){return Ok(false);}
        return Err(error).context("Inspect exact Windows update helper process");
    }
    let handle=unsafe{OwnedHandle::from_raw_handle(raw as _)};
    match unsafe{WaitForSingleObject(handle.as_raw_handle() as _,0)}{
        WAIT_OBJECT_0=>return Ok(false),
        WAIT_TIMEOUT=>{},
        _=>return Err(std::io::Error::last_os_error()).context("Inspect update helper exit state"),
    }
    let mut created=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    let mut exited=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    let mut kernel=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    let mut user=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    if unsafe{GetProcessTimes(handle.as_raw_handle() as _,&mut created,&mut exited,
            &mut kernel,&mut user)}==0{
        return Err(std::io::Error::last_os_error()).context("Verify update helper creation time");
    }
    let ticks=(u64::from(created.dwHighDateTime)<<32)|u64::from(created.dwLowDateTime);
    Ok(ticks==identity.start_ticks)
}
fn read_helper(directory:&Path,request:&Request)->Result<Option<HelperIdentity>>{
    let path=helper_path(directory,&request.request_id);
    if !path_exists(&path)?{return Ok(None);}
    let helper:HelperIdentity=serde_json::from_slice(&util::private_read(&path,1024)?)?;
    if helper.request_id!=request.request_id||!util::constant_eq(&helper.nonce,&request.nonce)
        ||helper.pid<=1||helper.start_ticks==0{
        bail!("Windows update helper record does not match its authorization");
    }
    Ok(Some(helper))
}
fn published_executable(install:&preflight::Installation,sha:&str)->Result<PathBuf>{
    if !preflight::digest_ok(sha){bail!("Invalid update release digest");}
    let version=install.root.join("releases").join(sha);
    preflight::real_directory(&install.root)?;
    preflight::real_directory(&install.root.join("releases"))?;
    if !version.exists(){fs::create_dir(&version)?;}
    preflight::real_directory(&version)?;
    Ok(version.join(EXE_NAME))
}
fn stage_binary(source:&Path,target:&Path,expected:&str)->Result<()>{
    if !preflight::digest_ok(expected){bail!("Invalid candidate binary checksum");}
    if target.exists(){
        let pinned=preflight::regular_file(target)?;
        if !crate::platform::single_link(&pinned){
            bail!("Installed Windows update release must have exactly one filesystem link");
        }
        if preflight::fingerprint(target)?.0!=expected{
            bail!("Previously installed update release has a different digest");
        }
        return Ok(());
    }
    let mut from=preflight::regular_file(source)?;
    let limit=from.metadata()?.len();
    if limit==0||limit>preflight::MAX_EXE_BYTES{bail!("Invalid staged update size");}
    let temporary=target.with_file_name(format!("endlessvibe-{}.tmp",util::random_secret()?));
    let mut to=OpenOptions::new().write(true).create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(&temporary)?;
    let result=(||->Result<()>{
        let count=std::io::copy(&mut (&mut from).take(preflight::MAX_EXE_BYTES+1),&mut to)?;
        if count!=limit{bail!("Source changed while staging Windows service update");}
        to.flush()?;to.sync_all()?;
        drop(to);
        if preflight::fingerprint(&temporary)?.0!=expected{
            bail!("Staged Windows service update does not match requested SHA-256");
        }
        if target.exists(){bail!("Windows update release was created concurrently");}
        move_new(&temporary,target)?;
        if preflight::fingerprint(target)?.0!=expected{
            bail!("Published Windows service update changed unexpectedly");
        }
        Ok(())
    })();
    if result.is_err(){let _=fs::remove_file(&temporary);}
    result
}
fn wide(path:&Path)->Vec<u16>{
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}
fn manifest_transaction_paths(manifest:&Path,token:&str)->Result<(PathBuf,PathBuf)>{
    if !valid_id(token){bail!("Invalid manifest transaction");}
    let root=manifest.parent().context("Windows update manifest has no parent")?;
    // A failed filesystem operation may include its path in an error chain.
    // Never put the one-use authorization nonce in a filename or public error.
    let tag=util::digest(token.as_bytes());
    Ok((root.join(format!("current-{tag}.tmp")),root.join(format!("previous-{tag}.json"))))
}
fn verify_manifest(manifest:&Path,expected:&str,config:&Path)->Result<()>{
    let value:preflight::InstalledManifest=serde_json::from_slice(
        &util::private_read(manifest,4096)?)?;
    if !value.sha256.eq_ignore_ascii_case(expected)||
        value.config_path.as_deref().is_some_and(|p|!p.is_empty()&&Path::new(p)!=config){
        bail!("Windows installation manifest does not match the verified release and config");
    }
    Ok(())
}
fn replace_manifest(manifest:&Path,new_sha:&str,config:&Path,token:&str)->Result<PathBuf>{
    if !preflight::digest_ok(new_sha)||!valid_id(token){bail!("Invalid manifest transaction");}
    let root=manifest.parent().context("Windows update manifest has no parent")?;
    preflight::real_directory(root)?;
    let (temporary,backup)=manifest_transaction_paths(manifest,token)?;
    if path_exists(&backup)?{bail!("Previous manifest recovery evidence already exists");}
    let bytes=serde_json::to_vec(&json!({
        "sha256":new_sha,
        "config_path":config,
    }))?;
    publish_record(&temporary,&bytes)?;
    if unsafe{ReplaceFileW(wide(manifest).as_ptr(),wide(&temporary).as_ptr(),
            wide(&backup).as_ptr(),0,std::ptr::null_mut(),std::ptr::null_mut())}==0{
        // In particular, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 may already
        // have moved the original to backup. Keep both files for recovery.
        return Err(std::io::Error::last_os_error())
            .context("Windows manifest replacement failed; transaction files were retained");
    }
    verify_manifest(manifest,new_sha,config)?;
    Ok(backup)
}
fn restore_previous_manifest(manifest:&Path,old_sha:&str,config:&Path,token:&str)->Result<()>{
    if path_exists(manifest)?{
        // Refuse to overwrite an unknown or concurrently changed manifest.
        return verify_manifest(manifest,old_sha,config)
            .context("Current manifest cannot be proven to name the previous release");
    }
    let (_,backup)=manifest_transaction_paths(manifest,token)?;
    verify_manifest(&backup,old_sha,config)
        .context("Previous manifest recovery backup could not be verified")?;
    let bytes=util::private_read(&backup,4096)?;
    publish_record(manifest,&bytes)
        .context("Restore verified previous manifest without overwriting another file")?;
    verify_manifest(manifest,old_sha,config)
}

#[derive(Debug)]
struct ServiceStartFailure{
    error:anyhow::Error,
    child_exited:bool,
}
impl ServiceStartFailure{
    fn not_started(error:anyhow::Error)->Self{Self{error,child_exited:true}}
}
fn child_exit_confirmed(result:std::io::Result<Option<std::process::ExitStatus>>)->bool{
    matches!(result,Ok(Some(_)))
}
fn spawn_service(exe:&Path,config:&Path,cwd:&Path,state_dir:&Path)
    ->std::result::Result<util::ProcessIdentity,ServiceStartFailure>{
    preflight::fingerprint(exe).map_err(ServiceStartFailure::not_started)?;
    let mut child=Command::new(exe)
        .arg("--config").arg(config)
        .current_dir(cwd)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(WINDOWS_DETACHED_PROCESS|WINDOWS_NEW_PROCESS_GROUP)
        .spawn().context("Start exact verified Windows EndlessVibe release")
        .map_err(ServiceStartFailure::not_started)?;
    let result=(||->Result<util::ProcessIdentity>{
        let deadline=Instant::now()+Duration::from_secs(12);
        while Instant::now()<deadline{
            if let Some(identity)=util::read_service_identity(state_dir)?{
                if identity.pid==child.id() as i32&&identity_alive(identity)?{
                    thread::sleep(Duration::from_millis(250));
                    if identity_alive(identity)?&&child.try_wait()?.is_none(){return Ok(identity);}
                }
            }
            if child.try_wait()?.is_some(){bail!("New Windows service exited before reporting readiness");}
            thread::sleep(Duration::from_millis(150));
        }
        bail!("Windows process did not publish a verified service identity within 12 seconds")
    })();
    match result{
        Ok(identity)=>Ok(identity),
        Err(error)=>Err(ServiceStartFailure{
            error,child_exited:child_exit_confirmed(child.try_wait()),
        }),
    }
}
fn graceful_stop(data_dir:&Path,identity:util::ProcessIdentity)->Result<()>{
    if !identity_alive(identity)?{
        bail!("Windows service PID/creation identity changed before update");
    }
    util::request_service_shutdown(data_dir,identity)?;
    let deadline=Instant::now()+Duration::from_secs(15);
    while identity_alive(identity)?&&Instant::now()<deadline{
        thread::sleep(Duration::from_millis(100));
    }
    if identity_alive(identity)?{
        bail!("Windows service did not stop gracefully; refusing forced termination");
    }
    util::clear_service_identity(data_dir,identity)?;
    Ok(())
}
// Roll back only when it is demonstrably safe to start the previous
// version. A still-running or identity-ambiguous child must be inspected by
// the owner instead of spawning two competing service instances.
fn require_service_fully_stopped(state_dir:&Path)->Result<()>{
    if let Some(identity)=util::read_service_identity(state_dir)?{
        if identity_alive(identity)?{
            bail!("An EndlessVibe process is still running; automatic rollback is unsafe");
        }
    }
    let _exclusive=util::single_instance(&state_dir.join("service.lock"))
        .context("Service.lock is still owned; refusing an overlapping rollback")?;
    Ok(())
}
fn rolled_back_report(request:&Request,identity:util::ProcessIdentity,error:&anyhow::Error)->Value{
    json!({
        "state":"rolled_back",
        "request_id":request.request_id,
        "current_sha256":request.old_sha256,
        "candidate_sha256":request.new_sha256,
        "running_pid":identity.pid,
        "running_start_ticks":identity.start_ticks,
        "message":util::bounded_text(&format!("{error:#}"),480),
        "manual_recovery_may_be_required":false,
        "rollback_required":false,
    })
}
fn helper_apply(settings:&Config,request:&Request)->Result<Value>{
    validate_request(request)?;
    if !settings.security.allow_windows_service_update{
        bail!("Windows service self-update opt-in was revoked");
    }
    let configured=settings.security.windows_update_project.as_ref()
        .context("No configured exact Windows update Project")?;
    if fs::canonicalize(configured)?!=fs::canonicalize(&request.project_root)?{
        bail!("Project identity changed since the authorized update request");
    }
    let installed=preflight::installation_from_executable(&std::env::current_exe()?)?;
    if installed.config_path.as_deref().is_some_and(|p|
        !p.is_empty()&&Path::new(p)!=request.config_path){
        bail!("Service config path differs from the installation manifest");
    }
    let permitted=settings.workspaces.iter().find(|w|w.id==request.workspace)
        .and_then(|w|w.projects.iter().find(|p|p.id==request.project))
        .filter(|p|p.allow_exec&&p.allow_write)
        .context("Project permission was revoked before self-update")?;
    if fs::canonicalize(&request.project_root)?!=fs::canonicalize(configured)?
        ||!settings.workspaces.iter().any(|w|w.id==request.workspace&&
            fs::canonicalize(w.path.join(&permitted.path)).ok()
               ==fs::canonicalize(&request.project_root).ok()){
        bail!("Configured Windows Project root or grant changed");
    }
    if installed.sha256!=request.old_sha256{
        bail!("Installed release differs from the explicitly approved previous hash");
    }
    let target=published_executable(&installed,&request.new_sha256)?;
    if preflight::fingerprint(&target)?.0!=request.new_sha256{
        bail!("Staged Windows release changed after original approval");
    }
    let current=util::read_service_identity(&settings.security.data_dir)?
        .context("Running service identity was not found")?;
    if current.pid!=request.old_pid||current.start_ticks!=request.old_start_ticks{
        bail!("Service PID/creation time changed before update; refusing stale request");
    }

    // Give the parent MCP response time to leave the TLS transport before
    // closing it. The helper is a separately detached, bounded process.
    thread::sleep(Duration::from_millis(1000));
    graceful_stop(&settings.security.data_dir,current)?;
    let next=spawn_service(&target,&request.config_path,&request.project_root,
        &settings.security.data_dir);
    let next=match next{
        Ok(value)=>value,
        Err(failure)=>{
            if !failure.child_exited{
                return Err(failure.error.context(
                    "New service child is still alive or its exit is unverified; automatic rollback refused"));
            }
            let error=failure.error;
            require_service_fully_stopped(&settings.security.data_dir)
                .context("Cannot verify new process exited; rollback requires manual inspection")?;
            verify_manifest(&installed.manifest,&request.old_sha256,&request.config_path)?;
            let recovery=spawn_service(&installed.running,&request.config_path,
                &request.project_root,&settings.security.data_dir);
            return match recovery{
                Ok(identity)=>Ok(rolled_back_report(request,identity,&error)),
                Err(restore)=>Err(error.context(format!(
                    "New service failed and previous version did not reach verified readiness: {:#}",restore.error))),
            };
        }
    };
    let manifest=replace_manifest(&installed.manifest,&request.new_sha256,
        &request.config_path,&request.nonce);
    let _backup=match manifest{
        Ok(backup)=>backup,
        Err(error)=>{
            graceful_stop(&settings.security.data_dir,next)
                .context("Manifest update failed but new service would not stop; manual recovery required")?;
            require_service_fully_stopped(&settings.security.data_dir)
                .context("Manifest rollback refused while another service holds the lock")?;
            restore_previous_manifest(&installed.manifest,&request.old_sha256,
                &request.config_path,&request.nonce)
                .context("Previous manifest recovery failed; do not restart an unverified release")?;
            let recovery=spawn_service(&installed.running,&request.config_path,
                &request.project_root,&settings.security.data_dir);
            return match recovery{
                Ok(identity)=>Ok(rolled_back_report(request,identity,&error)),
                Err(restore)=>Err(error.context(format!(
                    "Manifest transaction failed and previous release did not reach verified readiness: {:#}",restore.error))),
            };
        }
    };
    // Keep the old manifest as recovery evidence, including if the terminal
    // result cannot be persisted after the new process has started.
    Ok(json!({
        "state":"succeeded",
        "request_id":request.request_id,
        "current_sha256":request.new_sha256,
        "previous_sha256":request.old_sha256,
        "new_pid":next.pid,
        "running_pid":next.pid,
        "running_start_ticks":next.start_ticks,
        "rollback_required":false,
        "previous_manifest_retained":true,
        "manual_recovery_may_be_required":false,
    }))
}

fn terminal_matches(
    report:&Value,request:&Request,current:util::ProcessIdentity,running_sha:&str,
)->Result<()>{
    let expected=match report["state"].as_str(){
        Some("succeeded")=>&request.new_sha256,
        Some("rolled_back")=>&request.old_sha256,
        Some("failed") if report["recovery_verified"]==true=>&request.old_sha256,
        _=>bail!("Update did not produce a terminal outcome with verified service recovery"),
    };
    if report["request_id"]!=request.request_id||report["current_sha256"]!=*expected
        ||report["running_pid"].as_i64()!=Some(i64::from(current.pid))
        ||report["running_start_ticks"].as_u64()!=Some(current.start_ticks)
        ||report["manual_recovery_may_be_required"]!=false||running_sha!=expected.as_str(){
        bail!("Terminal update result does not match the running release and exact process identity");
    }
    Ok(())
}
fn verify_terminal_service(
    directory:&Path,request:&Request,exe:&Path,config_path:&Path,state_dir:&Path,
)->Result<Value>{
    if request.config_path!=config_path{bail!("Update belongs to a different installed configuration");}
    let report:Value=serde_json::from_slice(&util::private_read(
        &result_path(directory,&request.request_id),MAX_RECORD_BYTES)?)?;
    let install=preflight::installation_from_executable(exe)?;
    verify_manifest(&install.manifest,&install.sha256,config_path)?;
    let current=util::current_process_identity()?;
    if util::read_service_identity(state_dir)?!=Some(current)||!identity_alive(current)?{
        bail!("Current service identity is not the verified installed instance");
    }
    terminal_matches(&report,request,current,&install.sha256)?;
    Ok(report)
}
fn retire_record(path:&Path,request:&Request)->Result<()>{
    if !path_exists(path)?{return Ok(());}
    if read_request(path)?!=*request{
        bail!("Recovery record changed before retirement; manual inspection required");
    }
    fs::remove_file(path).context("Terminal result is saved but recovery record cleanup failed")
}
fn complete_transaction(
    directory:&Path,request:&Request,exe:&Path,config_path:&Path,state_dir:&Path,
    exact_helper:Option<util::ProcessIdentity>,
)->Result<()>{
    let helper=read_helper(directory,request)?.context("Exact helper identity record is missing")?;
    if exact_helper.is_some_and(|expected|expected!=helper.process()){
        bail!("Recorded update helper differs from the process launched by this service");
    }
    if identity_alive(helper.process())?||update_lock_held(directory)?{
        bail!("Update helper exit is not yet confirmed");
    }
    verify_terminal_service(directory,request,exe,config_path,state_dir)?;
    if pending_requests(directory)?.iter().any(|item|item!=request){
        bail!("Other unresolved update records prevent job admission from resuming");
    }
    // The helper never removes these records. Only the surviving service can
    // retire them after verifying helper exit and the installed terminal state.
    retire_record(&working_path(directory,&request.request_id),request)?;
    let active=directory.join(ACTIVE);
    if path_exists(&active)?{retire_record(&active,request)?;}
    Ok(())
}
fn child_identity(child:&Child)->Result<util::ProcessIdentity>{
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::FILETIME,System::Threading::GetProcessTimes};
    let mut created=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    let mut exited=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    let mut kernel=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    let mut user=FILETIME{dwLowDateTime:0,dwHighDateTime:0};
    if unsafe{GetProcessTimes(child.as_raw_handle() as _,&mut created,&mut exited,
            &mut kernel,&mut user)}==0{
        return Err(std::io::Error::last_os_error()).context("Read spawned helper creation time");
    }
    Ok(util::ProcessIdentity{pid:child.id() as i32,
        start_ticks:(u64::from(created.dwHighDateTime)<<32)|u64::from(created.dwLowDateTime)})
}
async fn monitor_spawned_helper(
    mut child:Child,guard:crate::tools::jobs::ServiceUpdateGuard,
    directory:PathBuf,request:Request,exe:PathBuf,config_path:PathBuf,state_dir:PathBuf,
){
    // Waiting on a blocking pool thread would keep the old Tokio runtime
    // alive while the helper waits for that same process to exit.
    let outcome=async{
        loop{
            match child.try_wait(){
                Ok(Some(_))=>break,
                Ok(None)=>tokio::time::sleep(Duration::from_millis(200)).await,
                Err(error)=>return Err(error).context("Inspect exact update helper exit"),
            }
        }
        let exact=child_identity(&child)?;
        complete_transaction(&directory,&request,&exe,&config_path,&state_dir,Some(exact))
    }.await;
    if let Err(error)=outcome{
        guard.leave_paused();
        tracing::warn!(request_id=%request.request_id,error=%error,
            "Windows update needs recovery; job admission remains paused");
    }
    // Normal Drop resumes admission only if permanent shutdown has not begun.
}
async fn monitor_startup_update(
    guard:crate::tools::jobs::ServiceUpdateGuard,directory:PathBuf,request:Request,
    exe:PathBuf,config_path:PathBuf,state_dir:PathBuf,
){
    let outcome=async{
        let identity_deadline=Instant::now()+Duration::from_secs(5);
        loop{
            match read_helper(&directory,&request)?{
                Some(helper) if !identity_alive(helper.process())?=>break,
                Some(_)=>{},
                None if Instant::now()<identity_deadline&&update_lock_held(&directory)?=>{},
                None=>bail!("Unresolved update has no verifiable helper identity"),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        complete_transaction(&directory,&request,&exe,&config_path,&state_dir,None)
    }.await;
    if let Err(error)=outcome{
        guard.leave_paused();
        tracing::warn!(request_id=%request.request_id,error=%error,
            "Startup retained an unresolved update; job admission remains paused");
    }
}

/// Called after this process has installed service.pid and before serving any
/// HTTP, Dashboard or Transfer request. Read-only tools remain available while
/// the helper finishes; jobs resume only after the exact terminal verification.
pub fn guard_startup(rt:Arc<crate::runtime::Runtime>)->Result<()>{
    let directory=update_dir(&rt.config.security.data_dir);
    if !path_exists(&directory)?{return Ok(());}
    let guard=rt.jobs.begin_service_update()?;
    let inspection=(||->Result<Option<Request>>{
        preflight::real_directory(&directory)?;
        let mut pending=pending_requests(&directory)?;
        if pending.is_empty(){
            if update_lock_held(&directory)?{bail!("Update lock is held without a recovery record");}
            return Ok(None);
        }
        if pending.len()!=1{bail!("Multiple unresolved Windows updates require manual inspection");}
        Ok(pending.pop())
    })();
    match inspection{
        Ok(None)=>drop(guard),
        Ok(Some(request))=>{
            let exe=match std::env::current_exe(){
                Ok(exe)=>exe,
                Err(error)=>{
                    guard.leave_paused();
                    tracing::warn!(%error,"Could not verify startup executable; jobs remain paused");
                    return Ok(());
                }
            };
            tokio::spawn(monitor_startup_update(guard,directory,request,exe,
                rt.config_path.clone(),rt.config.security.data_dir.clone()));
        }
        Err(error)=>{
            guard.leave_paused();
            tracing::warn!(%error,"Windows update recovery records require inspection; jobs remain paused");
        }
    }
    Ok(())
}
/// Only a previously authenticated service may create the one-use request.
/// The helper refuses arbitrary new executable paths or service names.
pub fn request(rt:&crate::runtime::Runtime,project:&crate::workspace::Project,
    args:&crate::tools::types::WindowsServiceUpdateArgs)->Result<Value>{
    if !args.confirm{bail!("Explicit confirm=true required for Windows service update");}
    let guard=rt.jobs.begin_service_update()?;
    let handle=tokio::runtime::Handle::try_current()
        .context("Windows updater requires the running service runtime")?;
    if !valid_id(&args.request_id)||!preflight::digest_ok(&args.expected_current_sha256)
        ||!preflight::digest_ok(&args.expected_candidate_sha256){
        bail!("Windows update requires a stable request_id and two full SHA-256 values");
    }
    let plan=preflight::plan_for_project(rt,project)?;
    if plan["enabled"]!=true{bail!("Windows self-update is disabled by the administrator");}
    if plan["current_sha256"]!=args.expected_current_sha256
        ||plan["candidate_sha256"]!=args.expected_candidate_sha256
        ||plan["same_binary"]==true{
        bail!("Windows binary changed or does not differ from the running release; review a fresh plan");
    }
    let install=preflight::installation_from_executable(&std::env::current_exe()?)?;
    let source=preflight::allowed_candidate(&project.root.path)?;
    let release=published_executable(&install,&args.expected_candidate_sha256)?;
    stage_binary(&source,&release,&args.expected_candidate_sha256)?;
    let old=util::read_service_identity(&rt.config.security.data_dir)?
        .context("Service identity unavailable for self-update")?;
    if old!=util::current_process_identity()?{
        bail!("Only the currently running, verified Windows service can start an update");
    }
    let nonce=util::random_secret()?;
    let pending=Request{
        nonce:nonce.clone(),
        request_id:args.request_id.clone(),
        workspace:project.workspace_id.clone(),
        project:project.config.id.clone(),
        old_sha256:args.expected_current_sha256.clone(),
        new_sha256:args.expected_candidate_sha256.clone(),
        config_path:rt.config_path.clone(),
        project_root:project.root.path.clone(),
        old_pid:old.pid,old_start_ticks:old.start_ticks,
        created_at_unix:util::now(),
    };
    validate_request(&pending)?;
    let updates=update_dir(&rt.config.security.data_dir);
    util::private_dir(&updates)?;
    let current=updates.join(ACTIVE);
    let unresolved=(||->Result<bool>{
        Ok(!pending_requests(&updates)?.is_empty()||update_lock_held(&updates)?)
    })();
    match unresolved{
        Ok(false)=>{},
        Ok(true)=>{
            guard.leave_paused();
            bail!("Another update is pending or interrupted; jobs remain paused for recovery");
        }
        Err(error)=>{
            guard.leave_paused();
            return Err(error).context("Update recovery state could not be inspected; jobs remain paused");
        }
    }
    if path_exists(&result_path(&updates,&args.request_id))?
       ||path_exists(&helper_path(&updates,&args.request_id))?{
        bail!("Windows service update request_id was used already; choose a new stable ID");
    }
    if let Err(error)=publish_record(&current,&serde_json::to_vec(&pending)?){
        if path_exists(&current).unwrap_or(true)||update_lock_held(&updates).unwrap_or(true){
            guard.leave_paused();
        }
        return Err(error).context("Windows update authorization could not be published");
    }
    let helper=Command::new(&install.running)
        .arg("--internal-windows-update")
        .arg(&rt.config_path)
        .arg(&nonce)
        .current_dir(&project.root.path)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(WINDOWS_DETACHED_PROCESS|WINDOWS_NEW_PROCESS_GROUP)
        .spawn();
    match helper{
        Ok(child)=>{
            let pid=child.id();
            handle.spawn(monitor_spawned_helper(child,guard,updates,pending,
                install.running,rt.config_path.clone(),rt.config.security.data_dir.clone()));
            Ok(json!({
                "accepted":true,
                "request_id":args.request_id,
                "expected_current_sha256":args.expected_current_sha256,
                "expected_new_sha256":args.expected_candidate_sha256,
                "helper_pid":pid,
                "status":"handoff_started",
                "shell_invoked":false,
                "job_admission_paused":true,
                "service_running_status_not_yet_confirmed":true,
            }))
        }
        Err(error)=>{
            // No helper exists, so this is the one failure path where the
            // original service can retire active.json without waiting on it.
            let report=json!({
                "state":"failed","request_id":pending.request_id,
                "current_sha256":pending.old_sha256,
                "running_pid":old.pid,"running_start_ticks":old.start_ticks,
                "recovery_verified":true,"manual_recovery_may_be_required":false,
                "message":"Windows updater helper could not start; service was not stopped",
            });
            let saved=(||->Result<()>{
                persist_report(&updates,&pending,&report)?;
                verify_terminal_service(&updates,&pending,&install.running,&rt.config_path,
                    &rt.config.security.data_dir)?;
                retire_record(&current,&pending)
            })();
            if let Err(recovery)=saved{
                guard.leave_paused();
                return Err(recovery).context("Helper did not start; recovery evidence retained and jobs remain paused");
            }
            Err(error).context("Windows updater helper could not start; service was not stopped")
        }
    }
}
/// Only the original versioned EXE may complete a prepared one-time request.
pub fn run_helper(config_path:&Path,nonce:&str)->Result<()>{
    if !config_path.is_absolute()||nonce.len()<32||nonce.len()>64||
       !nonce.bytes().all(|c|c.is_ascii_alphanumeric()||b"_-".contains(&c)){
        bail!("Invalid private Windows updater helper invocation");
    }
    let config=Config::load_file(config_path)?;
    if !config.security.allow_windows_service_update{
        bail!("Windows update was not enabled by this installation");
    }
    let updates=update_dir(&config.security.data_dir);
    preflight::real_directory(&updates)?;
    let _lock=util::single_instance(&updates.join("update.lock"))?;
    let active=updates.join(ACTIVE);
    let pending=read_request(&active)?;
    validate_request(&pending)?;
    if !util::constant_eq(&pending.nonce,nonce)||pending.config_path!=config_path{
        bail!("Windows update helper token or trusted config path does not match");
    }
    let claimed=working_path(&updates,&pending.request_id);
    move_new(&active,&claimed).context("Windows update authorization was consumed already")?;
    let identity=util::current_process_identity()?;
    let helper=HelperIdentity{request_id:pending.request_id.clone(),nonce:pending.nonce.clone(),
        pid:identity.pid,start_ticks:identity.start_ticks};
    publish_record(&helper_path(&updates,&pending.request_id),&serde_json::to_vec(&helper)?)
        .context("Helper identity could not be recorded; service was not stopped")?;
    let result=helper_apply(&config,&pending);
    let report=match &result{
        Ok(value)=>value.clone(),
        Err(error)=>json!({
            "state":"failed",
            "request_id":pending.request_id,
            "message":util::bounded_text(&format!("{error:#}"),480),
            "manual_recovery_may_be_required":true,
        }),
    };
    persist_report(&updates,&pending,&report)?;
    // Keep working until a service monitor observes THIS helper's exit,
    // verifies the running installation, and retires the recovery record.
    result.map(|_|())
}

fn recovery_status(request_id:&str,message:&str,terminal:Option<&Value>)->Value{
    let mut value=json!({
        "request_id":request_id,"state":"recovery_required","replay_safe":false,
        "job_admission_paused":true,"manual_recovery_may_be_required":true,
        "message":util::bounded_text(message,480),
    });
    if let Some(state)=terminal.and_then(|value|value["state"].as_str()){
        value["recorded_terminal_state"]=json!(state);
    }
    value
}
fn status_from_directory(directory:&Path,request_id:&str)->Result<Value>{
    if !path_exists(directory)?{
        return Ok(json!({"request_id":request_id,"state":"unknown","replay_safe":false}));
    }
    preflight::real_directory(directory)?;
    let file=result_path(directory,request_id);
    let report=if path_exists(&file)?{
        let report:Value=serde_json::from_slice(&util::private_read(&file,MAX_RECORD_BYTES)?)?;
        if report["request_id"]!=request_id||report["state"].as_str().is_none(){
            bail!("Terminal update record is incomplete or belongs to another request");
        }
        Some(report)
    }else{None};
    let working=working_path(directory,request_id);
    let pending=if path_exists(&working)?{
        let request=read_request(&working)?;
        if request.request_id!=request_id{bail!("Working update record has a different request ID");}
        Some((request,"in_progress"))
    }else{
        let active=directory.join(ACTIVE);
        if path_exists(&active)?{
            let request=read_request(&active)?;
            if request.request_id==request_id{Some((request,"pending"))}else{None}
        }else{None}
    };
    if let Some((request,state))=pending{
        match read_helper(directory,&request)?{
            Some(helper) if identity_alive(helper.process())?=>{
                return Ok(json!({"request_id":request_id,"state":state,
                    "job_admission_paused":true,"replay_safe":false}));
            }
            Some(_)=>return Ok(recovery_status(request_id,
                "The helper exited; the recovery record has not been verified and retired by the service",
                report.as_ref())),
            None if util::now().saturating_sub(request.created_at_unix)<=5=>{
                return Ok(json!({"request_id":request_id,"state":"pending",
                    "helper_identity_pending":true,"job_admission_paused":true,"replay_safe":false}));
            }
            None=>return Ok(recovery_status(request_id,
                "The update has no verifiable helper identity; automatic replay is unsafe",report.as_ref())),
        }
    }
    Ok(report.unwrap_or_else(||json!({"request_id":request_id,"state":"unknown","replay_safe":false})))
}
pub fn status(rt:&crate::runtime::Runtime,project:&crate::workspace::Project,
    request_id:&str)->Result<Value>{
    project.exec_allowed()?;
    if !valid_id(request_id){bail!("Invalid Windows service update request ID");}
    let permitted=rt.config.security.windows_update_project.as_ref()
        .and_then(|p|fs::canonicalize(p).ok())
        .zip(fs::canonicalize(&project.root.path).ok())
        .is_some_and(|(configured,actual)|configured==actual);
    if !rt.config.security.allow_windows_service_update||!permitted{
        bail!("Windows self-update status is not enabled for this Project");
    }
    let directory=update_dir(&rt.config.security.data_dir);
    Ok(match status_from_directory(&directory,request_id){
        Ok(status)=>status,
        Err(error)=>recovery_status(request_id,&format!("Update recovery state cannot be read safely: {error:#}"),None),
    })
}

#[cfg(test)]
mod tests{
    use super::*;
    fn sample_request()->Request{
        Request{
            nonce:"x".repeat(43),request_id:"safe_20261011".into(),
            workspace:"project".into(),project:"EndlessVibe".into(),
            old_sha256:"a".repeat(64),new_sha256:"b".repeat(64),
            config_path:PathBuf::from(r"C:\config\config.toml"),
            project_root:PathBuf::from(r"F:\project\EndlessVibe"),
            old_pid:1234,old_start_ticks:234567,
            created_at_unix:util::now(),
        }
    }
    #[test]
    fn update_requests_are_exact_and_confirmed(){
        let mut request=sample_request();
        assert!(validate_request(&request).is_ok());
        request.old_sha256=request.new_sha256.clone();
        assert!(validate_request(&request).is_err());
        request.old_sha256="a".repeat(64);
        request.request_id="../evil".into();
        assert!(validate_request(&request).is_err());
        request.request_id="safe_20261011".into();
        request.workspace="../system".into();
        assert!(validate_request(&request).is_err(),
            "A helper request cannot escape to an unapproved workspace");
        request.workspace="project".into();
        request.project="other/project".into();
        assert!(validate_request(&request).is_err(),
            "Helper cannot rewrite the originally approved project identity");
        request.project="EndlessVibe".into();
        request.nonce="short".into();
        assert!(validate_request(&request).is_err(),"Helper nonce must be 32..64 bytes");
        request.nonce="x".repeat(43);
        request.created_at_unix=util::now().saturating_sub(301);
        assert!(validate_request(&request).is_err(),
            "A stale one-time authorization cannot restart a service after five minutes");
        assert!(!valid_id("../../evil"));
        assert!(!valid_id(""));
        assert!(run_helper(Path::new(r"C:\config\config.toml"),"invalid").is_err(),
            "Internal helper entry must not start without a valid authorization token");
    }
    #[test]
    fn native_manifest_replacement_keeps_an_old_backup(){
        let dir=tempfile::tempdir().unwrap();
        let manifest=dir.path().join("current.json");
        util::private_create(&manifest,br#"{"sha256":"old"}"#).unwrap();
        let backup=replace_manifest(&manifest,&"a".repeat(64),
            Path::new(r"C:\config\config.toml"),"safe_123").unwrap();
        let value:Value=serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        assert_eq!(value["sha256"],"a".repeat(64));
        assert_eq!(fs::read(&backup).unwrap(),br#"{"sha256":"old"}"#);
    }
    #[test]
    fn staging_never_overwrites_a_different_release(){
        let dir=tempfile::tempdir().unwrap();
        let source=dir.path().join("candidate.exe");
        let dest=dir.path().join("release.exe");
        fs::write(&source,b"binary content").unwrap();
        let digest=util::digest(b"binary content");
        stage_binary(&source,&dest,&digest).unwrap();
        stage_binary(&source,&dest,&digest).unwrap();
        fs::write(&dest,b"swapped").unwrap();
        assert!(stage_binary(&source,&dest,&digest).is_err());
        assert!(stage_binary(&source,&dir.path().join("other.exe"),&"f".repeat(64)).is_err());
    }
    #[test]
    fn terminal_publication_is_complete_create_only_and_retains_recovery(){
        let dir=tempfile::tempdir().unwrap();
        let request=sample_request();
        let working=working_path(dir.path(),&request.request_id);
        publish_record(&working,&serde_json::to_vec(&request).unwrap()).unwrap();
        let report=json!({"state":"failed","request_id":request.request_id,
            "manual_recovery_may_be_required":true});
        persist_report(dir.path(),&request,&report).unwrap();
        let result=result_path(dir.path(),&request.request_id);
        let saved=util::private_read(&result,MAX_RECORD_BYTES).unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&saved).unwrap(),report);
        assert!(working.exists(),"Publishing a result alone must not retire a live helper's record");
        assert!(persist_report(dir.path(),&request,&json!({"state":"succeeded"})).is_err());
        assert_eq!(fs::read(&result).unwrap(),saved,"An earlier terminal result cannot be overwritten");
        assert!(working.exists());
    }
    #[test]
    fn partial_write_and_sync_failure_never_publish_a_terminal_record(){
        let dir=tempfile::tempdir().unwrap();
        let request=sample_request();
        let working=working_path(dir.path(),&request.request_id);
        publish_record(&working,&serde_json::to_vec(&request).unwrap()).unwrap();
        let result=result_path(dir.path(),&request.request_id);
        let bytes=br#"{"state":"succeeded"}"#;
        for written in [4,bytes.len()]{
            assert!(publish_record_with(&result,bytes,|file,bytes|{
                file.write_all(&bytes[..written])?;
                bail!("Injected write/sync failure");
            }).is_err());
            assert!(!result.exists(),"Partial or unsynced bytes must not appear at the result path");
            assert_eq!(read_request(&working).unwrap(),request);
        }
        fs::create_dir(&result).unwrap();
        assert!(persist_report(dir.path(),&request,&json!({"state":"failed"})).is_err());
        assert!(result.is_dir(),"A blocked final path must not be replaced");
        assert_eq!(read_request(&working).unwrap(),request);
    }
    #[test]
    fn manifest_partial_failure_restores_only_the_verified_previous_copy(){
        let dir=tempfile::tempdir().unwrap();
        let manifest=dir.path().join("current.json");
        let request=sample_request();
        let (temporary,backup)=manifest_transaction_paths(&manifest,&request.nonce).unwrap();
        let old=serde_json::to_vec(&json!({"sha256":request.old_sha256,
            "config_path":request.config_path})).unwrap();
        let new=serde_json::to_vec(&json!({"sha256":request.new_sha256,
            "config_path":request.config_path})).unwrap();
        // Model ReplaceFileW error 1177: original has moved to backup,
        // replacement still has its temporary name, current is absent.
        publish_record(&backup,&old).unwrap();
        publish_record(&temporary,&new).unwrap();
        restore_previous_manifest(&manifest,&request.old_sha256,&request.config_path,
            &request.nonce).unwrap();
        assert_eq!(fs::read(&manifest).unwrap(),old);
        assert_eq!(fs::read(&backup).unwrap(),old);
        assert_eq!(fs::read(&temporary).unwrap(),new);
        assert!(!backup.to_string_lossy().contains(&request.nonce));
        fs::write(&manifest,&new).unwrap();
        assert!(restore_previous_manifest(&manifest,&request.old_sha256,&request.config_path,
            &request.nonce).is_err(),"An unknown current manifest must not be overwritten");
        assert_eq!(fs::read(&manifest).unwrap(),new);
        fs::remove_file(&manifest).unwrap();
        fs::write(&backup,&new).unwrap();
        assert!(restore_previous_manifest(&manifest,&request.old_sha256,&request.config_path,
            &request.nonce).is_err(),"A mismatched backup must not authorize a rollback");
        assert!(!manifest.exists());
    }
    #[test]
    fn rollback_requires_confirmed_child_exit(){
        use std::os::windows::process::ExitStatusExt;
        assert!(child_exit_confirmed(Ok(Some(std::process::ExitStatus::from_raw(1)))));
        assert!(!child_exit_confirmed(Ok(None)),"A live child cannot authorize rollback");
        assert!(!child_exit_confirmed(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,"injected process inspection failure"))));
    }
    #[test]
    fn admission_resumes_only_for_the_exact_terminal_hash_and_process(){
        let request=sample_request();
        let current=util::ProcessIdentity{pid:1234,start_ticks:345678};
        let mut report=json!({"state":"succeeded","request_id":request.request_id,
            "current_sha256":request.new_sha256,"running_pid":current.pid,
            "running_start_ticks":current.start_ticks,"manual_recovery_may_be_required":false});
        assert!(terminal_matches(&report,&request,current,&request.new_sha256).is_ok());
        assert!(terminal_matches(&report,&request,current,&request.old_sha256).is_err());
        report["running_start_ticks"]=json!(current.start_ticks+1);
        assert!(terminal_matches(&report,&request,current,&request.new_sha256).is_err());
        report["running_start_ticks"]=json!(current.start_ticks);
        report["state"]=json!("failed");
        assert!(terminal_matches(&report,&request,current,&request.new_sha256).is_err());
        let rollback=rolled_back_report(&request,current,&anyhow::anyhow!("candidate failed"));
        assert!(terminal_matches(&rollback,&request,current,&request.old_sha256).is_ok());
        assert!(terminal_matches(&rollback,&request,current,&request.new_sha256).is_err());
    }
    #[test]
    fn exited_or_reused_helper_pid_reports_recovery_instead_of_in_progress(){
        let dir=tempfile::tempdir().unwrap();
        let request=sample_request();
        publish_record(&working_path(dir.path(),&request.request_id),
            &serde_json::to_vec(&request).unwrap()).unwrap();
        let current=util::current_process_identity().unwrap();
        let mut helper=HelperIdentity{request_id:request.request_id.clone(),
            nonce:request.nonce.clone(),pid:current.pid,start_ticks:current.start_ticks};
        let path=helper_path(dir.path(),&request.request_id);
        publish_record(&path,&serde_json::to_vec(&helper).unwrap()).unwrap();
        assert_eq!(status_from_directory(dir.path(),&request.request_id).unwrap()["state"],"in_progress");
        helper.start_ticks+=1;
        fs::write(&path,serde_json::to_vec(&helper).unwrap()).unwrap();
        let status=status_from_directory(dir.path(),&request.request_id).unwrap();
        assert_eq!(status["state"],"recovery_required");
        assert_eq!(status["replay_safe"],false);
        assert!(!status.to_string().contains(&request.nonce));
        assert!(working_path(dir.path(),&request.request_id).exists());
    }
    #[test]
    fn changed_recovery_record_is_never_retired(){
        let dir=tempfile::tempdir().unwrap();
        let request=sample_request();
        let path=working_path(dir.path(),&request.request_id);
        let mut changed=request.clone();
        changed.new_sha256="c".repeat(64);
        publish_record(&path,&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(retire_record(&path,&request).is_err());
        assert_eq!(read_request(&path).unwrap(),changed);
        retire_record(&path,&changed).unwrap();
        assert!(!path.exists());
    }
}
