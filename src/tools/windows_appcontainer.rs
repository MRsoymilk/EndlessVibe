//! Windows AppContainer identity, safe Project input staging, and native probes.
//!
//! The production launcher supports only the fixed hash_input self-check.
//! It cannot execute arbitrary Project commands. AppContainer denies network
//! capabilities by default; copies in the per-job profile avoid persistent
//! ACL changes to the original Project.
use anyhow::{bail,Context,Result};
use std::{
    ffi::{c_void,OsString},
    fs,
    io::Write,
    mem::{size_of,zeroed},
    os::windows::{ffi::OsStringExt,io::{AsRawHandle,FromRawHandle,OwnedHandle}},
    path::{Path,PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{LocalFree,WAIT_OBJECT_0,WAIT_TIMEOUT},
    Security::{
        FreeSid,GetTokenInformation,PSID,SECURITY_CAPABILITIES,TOKEN_QUERY,TokenIsAppContainer,
        Authorization::ConvertSidToStringSidW,
        Isolation::{CreateAppContainerProfile,DeleteAppContainerProfile,GetAppContainerFolderPath},
    },
    System::{
        Com::CoTaskMemFree,
        Threading::{
            CreateProcessW,DeleteProcThreadAttributeList,GetCurrentProcess,GetExitCodeProcess,
            InitializeProcThreadAttributeList,OpenProcessToken,ResumeThread,TerminateProcess,
            UpdateProcThreadAttribute,WaitForSingleObject,CREATE_SUSPENDED,
            CREATE_UNICODE_ENVIRONMENT,EXTENDED_STARTUPINFO_PRESENT,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,PROCESS_INFORMATION,STARTUPINFOEXW,
        },
    },
};

const MAX_STAGED_FILE:usize=2*1024*1024;

/// Output bytes are brokered back for explicit review. A separate authorized
/// write_file call is required before any source Project mutation.
#[derive(Debug)]
pub(super) struct OutputArtifact{
    pub(super) relative_path:String,
    pub(super) bytes:Vec<u8>,
    pub(super) sha256:String,
}


fn wide(s:&str)->Vec<u16>{s.encode_utf16().chain(std::iter::once(0)).collect()}
unsafe fn read_wide_pointer(pointer:*const u16)->Result<OsString>{
    if pointer.is_null(){bail!("Windows returned a null AppContainer path");}
    let mut len=0usize;
    while len<32768&&unsafe{*pointer.add(len)}!=0{len+=1;}
    if len==32768{bail!("Windows returned an oversized AppContainer path");}
    Ok(OsString::from_wide(unsafe{std::slice::from_raw_parts(pointer,len)}))
}
struct LocalWide(*mut u16);
impl Drop for LocalWide{
    fn drop(&mut self){if !self.0.is_null(){unsafe{LocalFree(self.0 as _)};}}
}
struct ComWide(*mut u16);
impl Drop for ComWide{
    fn drop(&mut self){if !self.0.is_null(){unsafe{CoTaskMemFree(self.0 as _)}};}
}

pub(super) struct Profile{
    name:Vec<u16>,
    sid:PSID,
    journal:Option<PathBuf>,
}
impl Profile{
    #[cfg(test)]
    pub(super) fn create()->Result<Self>{
        let random=crate::util::random_secret()?;
        let name=format!("EndlessVibe.Isolated.{}",&crate::util::digest(random)[..32]);
        Self::create_named(&name,None)
    }

    /// Persist the profile identity *before* creating it so a service crash
    /// never leaves an untracked lowbox profile registered in Windows.
    fn create_journaled(journal_dir:&Path)->Result<Self>{
        crate::util::private_dir(journal_dir)?;
        let random=crate::util::random_secret()?;
        let name=format!("EndlessVibe.Isolated.{}",&crate::util::digest(random)[..32]);
        let marker=crate::tools::windows_recovery::mark_pending(journal_dir,&name)?;
        // If native profile creation returns an ambiguous failure, keep the
        // marker for the next exclusively locked service recovery.
        Self::create_named(&name,Some(marker))
    }
    fn create_named(name:&str,journal:Option<PathBuf>)->Result<Self>{
        let profile_name=wide(name);
        let display=wide("EndlessVibe isolated workload");
        let description=wide("Unique sandbox profile with no default network capabilities");
        let mut sid:PSID=ptr::null_mut();
        let hr=unsafe{CreateAppContainerProfile(
            profile_name.as_ptr(),display.as_ptr(),description.as_ptr(),
            ptr::null(),0,&mut sid,
        )};
        if hr<0{bail!("CreateAppContainerProfile failed: HRESULT 0x{:08x}",hr as u32);}
        if sid.is_null(){
            let _=unsafe{DeleteAppContainerProfile(profile_name.as_ptr())};
            bail!("AppContainer profile returned no SID");
        }
        Ok(Self{name:profile_name,sid,journal})
    }
    pub(super) fn capabilities(&self)->SECURITY_CAPABILITIES{
        SECURITY_CAPABILITIES{
            AppContainerSid:self.sid,Capabilities:ptr::null_mut(),
            CapabilityCount:0,Reserved:0,
        }
    }
    /// The returned directory is *owned* by this unique AppContainer package.
    /// Original Project ACLs are never modified for this staging operation.
    pub(super) fn folder(&self)->Result<PathBuf>{
        let mut raw_sid: *mut u16=ptr::null_mut();
        if unsafe{ConvertSidToStringSidW(self.sid,&mut raw_sid)}==0{
            return Err(std::io::Error::last_os_error()).context("Render AppContainer SID");
        }
        let sid=LocalWide(raw_sid);
        let mut raw_dir:*mut u16=ptr::null_mut();
        let hr=unsafe{GetAppContainerFolderPath(sid.0,&mut raw_dir)};
        if hr<0{
            bail!("GetAppContainerFolderPath failed: HRESULT 0x{:08x}",hr as u32);
        }
        let directory=ComWide(raw_dir);
        let value=unsafe{read_wide_pointer(directory.0)}?;
        let result=PathBuf::from(value);
        if !result.is_absolute(){bail!("AppContainer folder is not absolute");}
        Ok(result)
    }
    /// Bounded, capability-rooted file copy. Only explicitly selected files
    /// can cross into a job's private profile; no symlinks or sensitive names.
    pub(super) fn stage_project_file(
        &self,source:&crate::security::paths::Root,relative:&str,
    )->Result<PathBuf>{
        let relative_path=crate::security::paths::relative(relative,false)?;
        let bytes=source.read(relative,MAX_STAGED_FILE)?;
        let stage=self.folder()?.join("LocalState").join("EndlessVibe").join("inputs");
        let dest=stage.join(&relative_path);
        let parent=dest.parent().context("Missing staged file parent")?;
        fs::create_dir_all(parent)?;
        let mut output=fs::OpenOptions::new().write(true).create_new(true)
            .open(&dest).context("Create private AppContainer input file")?;
        output.write_all(&bytes)?;
        output.sync_all()?;
        Ok(dest)
    }

    /// Prepare a fresh result directory before starting untrusted child code.
    pub(super) fn prepare_results(&self)->Result<PathBuf>{
        let directory=self.folder()?.join("LocalState/EndlessVibe/outputs");
        fs::create_dir_all(&directory).context("Prepare AppContainer output directory")?;
        let root=crate::security::paths::Root::open(&self.folder()?)?;
        root.directory_path("LocalState/EndlessVibe/outputs")
    }
    /// Explicitly collect one file using a pinned capability-rooted reader.
    /// No output is automatically copied into the original Project directory.
    pub(super) fn collect_result(&self,relative:&str,max_bytes:usize)->Result<OutputArtifact>{
        if max_bytes==0||max_bytes>MAX_STAGED_FILE{
            bail!("AppContainer output limit must be between 1 byte and 2 MiB");
        }
        let relative_path=crate::security::paths::relative(relative,false)?;
        let profile=crate::security::paths::Root::open(&self.folder()?)?;
        let output_dir=profile.directory_path("LocalState/EndlessVibe/outputs")?;
        let output_root=crate::security::paths::Root::open(&output_dir)?;
        let bytes=output_root.read(relative,max_bytes)?;
        let sha256=crate::util::digest(&bytes);
        Ok(OutputArtifact{relative_path:relative_path.to_string_lossy().into_owned(),bytes,sha256})
    }
}
impl Profile{
    /// Complete cleanup after the child and all private file handles exit.
    /// A completed self-check must never report success with pending journals.
    fn finish(&mut self)->Result<()>{
        if self.name.is_empty(){return Ok(());}
        if !self.sid.is_null(){unsafe{FreeSid(self.sid)};self.sid=ptr::null_mut();}
        const PROFILE_NOT_FOUND:u32=0x80070490;
        let mut deleted=false;
        let mut code=0i32;
        for attempt in 0..8{
            code=unsafe{DeleteAppContainerProfile(self.name.as_ptr())};
            if code>=0||code as u32==PROFILE_NOT_FOUND{
                deleted=true;
                break;
            }
            if attempt<7{std::thread::sleep(std::time::Duration::from_millis(100));}
        }
        if !deleted{
            bail!("AppContainer cleanup deferred to protected recovery: HRESULT 0x{:08x}",code as u32);
        }
        if let Some(path)=&self.journal{
            fs::remove_file(path).context("Clear verified AppContainer profile journal")?;
        }
        self.name.clear();
        Ok(())
    }
}
impl Drop for Profile{
    fn drop(&mut self){
        if let Err(error)=self.finish(){
            eprintln!("AppContainer profile cleanup deferred: {error:#}");
        }
    }
}

// The production recovery implementation also serves as the native
// test oracle so behavior cannot drift between test and service startup.
#[cfg(test)]
fn recover_orphan_profiles(journal_dir:&Path)->Result<usize>{
    crate::tools::windows_recovery::recover_pending(journal_dir)
}

struct Attributes{storage:Vec<usize>}
impl Attributes{
    fn create(caps:&SECURITY_CAPABILITIES)->Result<Self>{
        let mut bytes=0usize;
        unsafe{InitializeProcThreadAttributeList(ptr::null_mut(),1,0,&mut bytes)};
        if bytes==0{bail!("Windows provided no AppContainer attribute storage size");}
        let mut storage=vec![0usize;bytes.div_ceil(size_of::<usize>())];
        let list=storage.as_mut_ptr() as _;
        if unsafe{InitializeProcThreadAttributeList(list,1,0,&mut bytes)}==0{
            return Err(std::io::Error::last_os_error()).context("Initialize AppContainer attributes");
        }
        if unsafe{UpdateProcThreadAttribute(
            list,0,PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            caps as *const _ as _,size_of::<SECURITY_CAPABILITIES>(),
            ptr::null_mut(),ptr::null(),
        )}==0{
            unsafe{DeleteProcThreadAttributeList(list)};
            return Err(std::io::Error::last_os_error()).context("Install AppContainer capabilities");
        }
        Ok(Self{storage})
    }
    fn ptr(&mut self)->*mut c_void{self.storage.as_mut_ptr() as _}
}
impl Drop for Attributes{
    fn drop(&mut self){unsafe{DeleteProcThreadAttributeList(self.storage.as_mut_ptr() as _)};}
}

fn has_appcontainer_token(process:windows_sys::Win32::Foundation::HANDLE)->Result<bool>{
    let mut raw=ptr::null_mut();
    if unsafe{OpenProcessToken(process,TOKEN_QUERY,&mut raw)}==0{
        return Err(std::io::Error::last_os_error()).context("Open process token");
    }
    let token=unsafe{OwnedHandle::from_raw_handle(raw as _)};
    let mut flag=0u32;
    let mut read=0u32;
    if unsafe{GetTokenInformation(
        token.as_raw_handle() as _,TokenIsAppContainer,&mut flag as *mut _ as _,
        size_of::<u32>() as u32,&mut read,
    )}==0{
        return Err(std::io::Error::last_os_error()).context("Check AppContainer token");
    }
    Ok(flag!=0)
}

fn sandbox_environment(profile:&Profile,extras:&[(&str,String)])->Result<Vec<u16>>{
    let root=profile.folder()?;
    let temp=root.join("Temp");
    fs::create_dir_all(&temp)?;
    // Derive Windows loader environment strictly from the OS APIs. Never
    // trust a parent process's SystemRoot, SystemDrive, PATH or ComSpec.
    let system_dir=crate::tools::windows_trusted_worker::system32_directory()?;
    let system_root=system_dir.parent()
        .context("Windows System32 path has no parent")?
        .to_string_lossy().into_owned();
    let drive=match system_dir.components().next(){
        Some(std::path::Component::Prefix(prefix))=>match prefix.kind(){
            std::path::Prefix::Disk(letter)|std::path::Prefix::VerbatimDisk(letter)=>letter,
            _=>bail!("Windows System32 must be on a local disk volume"),
        },
        _=>bail!("Windows System32 path is missing its disk prefix"),
    };
    let system_drive=format!("{}:",(drive as char).to_ascii_uppercase());
    let mut values=vec![
        ("SystemRoot".to_owned(),system_root.clone()),
        ("WINDIR".to_owned(),system_root),
        ("SystemDrive".to_owned(),system_drive),
        ("PATH".to_owned(),system_dir.to_string_lossy().into_owned()),
        ("LOCALAPPDATA".to_owned(),root.to_string_lossy().into_owned()),
        ("APPDATA".to_owned(),root.to_string_lossy().into_owned()),
        ("USERPROFILE".to_owned(),root.to_string_lossy().into_owned()),
        ("TEMP".to_owned(),temp.to_string_lossy().into_owned()),
        ("TMP".to_owned(),temp.to_string_lossy().into_owned()),
    ];
    for (key,value) in extras{
        let name=*key;
        let mut chars=name.bytes();
        let safe_name=matches!(chars.next(),Some(b)if b.is_ascii_alphabetic()||b==b'_')
            &&chars.all(|b|b.is_ascii_alphanumeric()||b==b'_');
        if !safe_name||name.len()>128||value.contains('\0')||value.len()>4096
            ||!name.starts_with("ENDLESSVIBE_"){
            bail!("Invalid AppContainer worker environment variable");
        }
        if values.iter().any(|(existing,_):&(String,String)|existing.eq_ignore_ascii_case(name)){
            bail!("AppContainer worker environment cannot override a protected variable");
        }
        values.push((name.to_owned(),value.clone()));
    }
    values.sort_by_key(|entry|entry.0.to_ascii_lowercase());
    let mut output=Vec::new();
    for (key,value) in values{
        output.extend(format!("{key}={value}").encode_utf16());
        output.push(0);
    }
    output.push(0);
    Ok(output)
}

#[derive(Debug)]
struct ProbeOutcome{exit_code:u32,cancelled:bool,timed_out:bool}
struct ProbeProcess{process:OwnedHandle,thread:OwnedHandle}
impl ProbeProcess{
    fn resume_and_wait(
        &self,job:&crate::tools::windows_job::JobObject,
        timeout:std::time::Duration,cancel:&std::sync::atomic::AtomicBool,
        mut logs:Option<&mut BoundedLogBroker>,
        mut gate:Option<&mut crate::tools::windows_loaded_modules::LoaderGate>,
        pid:u32,
    )->Result<ProbeOutcome>{
        use std::sync::atomic::Ordering;
        if unsafe{ResumeThread(self.thread.as_raw_handle() as _)}==u32::MAX{
            return Err(std::io::Error::last_os_error()).context("Resume isolated probe");
        }
        let deadline=std::time::Instant::now()+timeout;
        let(mut cancelled,mut timed_out)=(false,false);
        loop{
            if let Some(verifier)=gate.as_deref_mut(){
                if let Err(error)=verifier.observe(pid){
                    job.terminate();
                    return Err(error).context("AppContainer loaded-module identity rejected; Job Object terminated");
                }
            }
            if let Some(broker)=logs.as_deref_mut(){
                if let Err(error)=broker.poll(){
                    job.terminate();
                    return Err(error).context("AppContainer output rejected; contained Job Object was terminated");
                }
            }
            if cancel.load(Ordering::SeqCst){cancelled=true;break;}
            if std::time::Instant::now()>=deadline{timed_out=true;break;}
            match unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,30)}{
                WAIT_OBJECT_0=>break,
                WAIT_TIMEOUT=>{},
                _=>return Err(std::io::Error::last_os_error()).context("Wait for isolated test child"),
            }
        }
        // Terminate the entire job even on successful completion: no orphaned
        // grandchildren or leaked handles may remain after the task finishes.
        job.terminate();
        match unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,5_000)}{
            WAIT_OBJECT_0=>{},
            WAIT_TIMEOUT=>bail!("AppContainer process survived Job Object termination"),
            _=>return Err(std::io::Error::last_os_error()).context("Wait for contained process exit"),
        }
        if let Some(broker)=logs.as_deref_mut(){broker.poll()?;}
        let mut code=0u32;
        if unsafe{GetExitCodeProcess(self.process.as_raw_handle() as _,&mut code)}==0{
            return Err(std::io::Error::last_os_error()).context("Read isolated process exit code");
        }
        Ok(ProbeOutcome{exit_code:code,cancelled,timed_out})
    }
}
impl Drop for ProbeProcess{
    fn drop(&mut self){
        if unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,0)}==WAIT_TIMEOUT{
            let _=unsafe{TerminateProcess(self.process.as_raw_handle() as _,1)};
            let _=unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,5_000)};
        }
    }
}

#[cfg(test)]
fn launch_controlled(
    profile:&Profile,program:&Path,args:&str,extras:&[(&str,String)],
    timeout:std::time::Duration,cancel:&std::sync::atomic::AtomicBool,
    logs:Option<&mut BoundedLogBroker>,
)->Result<ProbeOutcome>{
    launch_controlled_with_gate(profile,program,args,extras,timeout,cancel,logs,None)
}
fn launch_controlled_with_gate(
    profile:&Profile,program:&Path,args:&str,extras:&[(&str,String)],
    timeout:std::time::Duration,cancel:&std::sync::atomic::AtomicBool,
    logs:Option<&mut BoundedLogBroker>,
    gate:Option<&mut crate::tools::windows_loaded_modules::LoaderGate>,
)->Result<ProbeOutcome>{
    let caps=profile.capabilities();
    let mut attrs=Attributes::create(&caps)?;
    let executable=wide(&program.to_string_lossy());
    let mut command=wide(&format!("\"{}\" {args}",program.display()));
    let root=profile.folder()?;
    let cwd=wide(&root.to_string_lossy());
    let mut environment=sandbox_environment(profile,extras)?;
    let mut startup:STARTUPINFOEXW=unsafe{zeroed()};
    startup.StartupInfo.cb=size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList=attrs.ptr() as _;
    let job=crate::tools::windows_job::JobObject::create(512,8)?;
    let mut child:PROCESS_INFORMATION=unsafe{zeroed()};
    let ok=unsafe{CreateProcessW(
        executable.as_ptr(),command.as_mut_ptr(),ptr::null(),ptr::null(),0,
        CREATE_SUSPENDED|EXTENDED_STARTUPINFO_PRESENT|CREATE_UNICODE_ENVIRONMENT,
        environment.as_mut_ptr() as _,cwd.as_ptr(),&startup.StartupInfo,&mut child,
    )};
    if ok==0{return Err(std::io::Error::last_os_error()).context("Launch isolated test process");}
    let process=ProbeProcess{
        process:unsafe{OwnedHandle::from_raw_handle(child.hProcess as _)},
        thread:unsafe{OwnedHandle::from_raw_handle(child.hThread as _)},
    };
    if !has_appcontainer_token(process.process.as_raw_handle() as _)?{
        bail!("Isolated child did not have a real AppContainer token");
    }
    job.assign(child.dwProcessId)?;
    if !job.contains(child.dwProcessId)?{bail!("AppContainer worker escaped its Job Object");}
    process.resume_and_wait(&job,timeout,cancel,logs,gate,child.dwProcessId)
}

#[cfg(test)]
fn launch_controlled_test(
    profile:&Profile,program:&Path,args:&str,extras:&[(&str,String)],
    timeout:std::time::Duration,cancel:&std::sync::atomic::AtomicBool,
)->Result<ProbeOutcome>{
    launch_controlled(profile,program,args,extras,timeout,cancel,None)
}

#[cfg(test)]
fn launch_isolated_test(
    profile:&Profile,program:&Path,args:&str,extras:&[(&str,String)],
)->Result<u32>{
    let outcome=launch_controlled_test(
        profile,program,args,extras,
        std::time::Duration::from_secs(10),
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    if outcome.cancelled||outcome.timed_out{
        bail!("AppContainer isolated probe exceeded its time limit");
    }
    Ok(outcome.exit_code)
}


// Fixed two-phase loader gate used only by the trusted hash_input Worker.
// The child waits for its parent to inspect live mapped modules before doing
// any input work, then waits for a second inspection before returning.
fn worker_loader_marker(root:&cap_std::fs::Dir,name:&str,bytes:&[u8])->Result<()>{
    use cap_std::fs::OpenOptions;
    let outputs=root.open_dir("outputs")?;
    let mut options=OpenOptions::new();
    options.write(true).create_new(true);
    let mut file=outputs.open_with(name,&options)?.into_std();
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn worker_loader_wait(root:&cap_std::fs::Dir,name:&str,required:&[u8])->Result<()>{
    use std::io::Read;
    for _ in 0..400{
        match root.open(name){
            Ok(file)=>{
                let file=file.into_std();
                if !crate::platform::single_link(&file){
                    bail!("AppContainer loader authorization is not a single-link file");
                }
                let mut bytes=Vec::new();
                file.take(32).read_to_end(&mut bytes)?;
                if bytes!=required{
                    bail!("AppContainer loader authorization contains unexpected data");
                }
                return Ok(());
            },
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{
                std::thread::sleep(std::time::Duration::from_millis(20));
            },
            Err(error)=>return Err(error).context("Read AppContainer module gate"),
        }
    }
    bail!("AppContainer module gate was not authorized in time")
}

/// Internal worker entrypoint. Never parses normal server CLI arguments, owner
/// credentials or Workspace configuration. The AppContainer token is mandatory.
pub fn run_fixed_worker_from_env()->Result<()>{
    use cap_std::{ambient_authority,fs::{Dir,OpenOptions}};
    use crate::tools::sandbox_protocol::{
        WorkerRequest,WorkerReceipt,MAX_INPUT_BYTES,MAX_REQUEST_BYTES,RECEIPT_NAME,
    };
    use std::io::Read;
    if !has_appcontainer_token(unsafe{GetCurrentProcess()})?{
        bail!("Internal sandbox worker requires a real AppContainer token");
    }
    let path=std::env::var_os("ENDLESSVIBE_PROTOCOL_ROOT")
        .context("Internal sandbox worker has no private root")?;
    let root_path=Path::new(&path);
    if !root_path.is_absolute()||!root_path.ends_with("LocalState/EndlessVibe"){
        bail!("Internal sandbox worker requires its private AppContainer staging root");
    }
    // Windows lowbox can open these files but cannot canonicalize its
    // privileged package ancestors. Start from one opened directory handle.
    let root=Dir::open_ambient_dir(root_path,ambient_authority())
        .context("Open isolated worker's private directory handle")?;
    let gated=std::env::var("ENDLESSVIBE_LOADER_GATE").ok().as_deref()==Some("1");
    if gated{
        worker_loader_marker(&root,"worker-loader-ready.ack",b"ready")?;
        worker_loader_wait(&root,"worker-loader-allow.ack",b"allow")?;
    }
    if root.symlink_metadata("worker-request.json")?.file_type().is_symlink(){
        bail!("Sandbox request cannot be a symlink");
    }
    let request_file=root.open("worker-request.json")?.into_std();
    if !crate::platform::single_link(&request_file){
        bail!("Sandbox request must be a single-link file");
    }
    let mut request_data=Vec::new();
    request_file.take((MAX_REQUEST_BYTES+1) as u64)
        .read_to_end(&mut request_data)?;
    let request=WorkerRequest::decode(&request_data)?;
    // Strict path grammar plus directory-handle traversal; each component
    // must be a real file or directory, not a reparse point.
    let mut current=PathBuf::new();
    let segments=request.input.split('/').collect::<Vec<_>>();
    for (index,part) in segments.iter().enumerate(){
        current.push(part);
        let metadata=root.symlink_metadata(&current)?;
        if metadata.file_type().is_symlink(){
            bail!("AppContainer input path contains a symlink");
        }
        if index+1<segments.len()&&!metadata.is_dir(){
            bail!("AppContainer input parent is not a directory");
        }
    }
    let input_file=root.open(&request.input)?.into_std();
    if !input_file.metadata()?.is_file()||!crate::platform::single_link(&input_file){
        bail!("AppContainer input must be a single-link regular file");
    }
    let mut input=Vec::new();
    input_file.take((MAX_INPUT_BYTES+1) as u64).read_to_end(&mut input)?;
    let receipt=WorkerReceipt::for_input(&request,&input)?;
    if root.symlink_metadata("outputs")?.file_type().is_symlink(){
        bail!("Sandbox output directory cannot be a symlink");
    }
    let outputs=root.open_dir("outputs")?;
    let mut options=OpenOptions::new();
    options.write(true).create_new(true);
    let bytes=receipt.encode()?;
    let mut file=outputs.open_with(RECEIPT_NAME,&options)?.into_std();
    file.write_all(&bytes)?;
    file.sync_all()?;
    // The sandbox itself owns these log files. The privileged broker neither
    // passes inheritable handles nor lets logs escape the private profile.
    let mut stdout=outputs.open_with("child-stdout.log",&options)?.into_std();
    stdout.write_all(b"hash_input: receipt generated\n")?;
    stdout.sync_all()?;
    let stderr=outputs.open_with("child-stderr.log",&options)?.into_std();
    stderr.sync_all()?;
    if gated{
        worker_loader_marker(&root,"worker-loader-done.ack",b"done")?;
        worker_loader_wait(&root,"worker-loader-release.ack",b"release")?;
    }
    Ok(())
}

/// Explicit local self-check of the production fixed-operation worker.
/// This is intentionally NOT a selectable backend for run_command or run_shell:
/// only a synthetic, private, non-secret sample is ever staged into a new
/// lowbox profile with no network capabilities.
pub fn probe_fixed_operation(state_dir:&Path)->Result<serde_json::Value>{
    use crate::tools::sandbox_protocol::{WorkerRequest,WorkerReceipt,RECEIPT_NAME};
    let temporary=state_dir.join("tmp");
    crate::util::private_dir(&temporary)?;
    let sample=tempfile::Builder::new().prefix("appcontainer-probe-")
        .tempdir_in(&temporary).context("Create private sample workspace")?;
    let sample_data=b"EndlessVibe isolated fixed-operation hash probe\n";
    fs::write(sample.path().join("probe.txt"),sample_data)?;
    let project=crate::security::paths::Root::open(sample.path())?;
    let journal=crate::tools::windows_recovery::journal_dir(state_dir);
    let mut profile=Profile::create_journaled(&journal)?;
    let staged=profile.stage_project_file(&project,"probe.txt")?;
    if !staged.is_file(){bail!("Sandbox input was not staged");}
    let output_dir=profile.prepare_results()?;
    let mut logs=BoundedLogBroker::open(&output_dir,MAX_PRODUCTION_LOG_BYTES)?;
    let private=profile.folder()?.join("LocalState/EndlessVibe");
    let request=WorkerRequest::new("local_self_check","inputs/probe.txt")?;
    crate::util::private_create(
        &private.join("worker-request.json"),&request.encode()?,
    )?;
    let trusted=crate::tools::windows_trusted_worker::TrustedWorker::stage_current(&private)?;
    trusted.verify()?;
    let mut gate=crate::tools::windows_loaded_modules::LoaderGate::new(&private,trusted.path())?;
    let cancel=std::sync::atomic::AtomicBool::new(false);
    let run=launch_controlled_with_gate(
        &profile,trusted.path(),"--internal-sandbox-worker",
        &[
            ("ENDLESSVIBE_PROTOCOL_ROOT",private.to_string_lossy().into_owned()),
            ("ENDLESSVIBE_LOADER_GATE","1".into()),
        ],
        std::time::Duration::from_secs(15),&cancel,Some(&mut logs),Some(&mut gate),
    )?;
    if run.cancelled||run.timed_out||run.exit_code!=0{
        bail!("Fixed AppContainer worker failed: exit={}, cancelled={}, timed_out={}",
            run.exit_code,run.cancelled,run.timed_out);
    }
    let loaded_modules=gate.summary()
        .context("Require pre- and post-operation live Windows module identity checks")?;
    trusted.verify().context("Verify lowbox Worker EXE after execution")?;
    let signature_diagnostics=crate::tools::windows_signature::runtime_diagnostics(
        &trusted.msvc_runtimes()
    )?;
    let log_report=logs.finish()?;
    let artifact=profile.collect_result(RECEIPT_NAME,
        crate::tools::sandbox_protocol::MAX_RECEIPT_BYTES)?;
    let receipt=WorkerReceipt::decode(&artifact.bytes)?;
    if artifact.relative_path!=RECEIPT_NAME{bail!("Unexpected sandbox output artifact");}
    let independently_read=project.read("probe.txt",
        crate::tools::sandbox_protocol::MAX_INPUT_BYTES)?;
    receipt.verify_against(&request,&independently_read)?;
    let result=serde_json::json!({
        "status":"passed",
        "worker":"appcontainer_fixed_operation",
        "operation":"hash_input",
        "input_bytes":receipt.input_bytes,
        "input_sha256":receipt.sha256,
        "receipt_sha256":artifact.sha256,
        "logs":log_report,
        "loaded_modules":loaded_modules,
        "signature_diagnostics":signature_diagnostics,
        "trusted_worker":{"origin":"current_endlessvibe_executable","sha256":trusted.sha256(),
                          "bytes":trusted.bytes(),"verified_before_and_after":true,
                          "arbitrary_project_executables":false,
                          "dependency_audit":{"format":"pe32_plus_amd64",
                              "approved_import_modules":trusted.imports(),
                              "msvc_runtime_modules_verified_in_system32":trusted.msvc_runtimes(),
                              "static_import_allowlist":true,
                              "system32_runtime_integrity_rechecked":true,
                              "delay_loaded_modules":0,
                              "unknown_modules_allowed":false,
                              "relative_dll_paths_allowed":false}},
        "protocol_version":crate::tools::sandbox_protocol::VERSION,
        "appcontainer_token_verified":true,
        "job_object_enforced":true,
        "network_capabilities_granted":0,
        "project_source_unmodified":true,
        "arbitrary_commands_supported":false,
        "general_execution_backend":false,
        "profile_crash_recovery_journaled":true,
        "profile_cleanup_verified":true,
    });
    // Windows may retain the loaded EXE section briefly after process exit.
    // Close all broker directory handles before explicit profile deletion.
    drop(logs);
    drop(gate);
    drop(trusted);
    profile.finish().context("Complete and verify AppContainer profile cleanup")?;
    Ok(result)
}

/// Append-only raw-byte log cursor. The parent owns this state; child-provided
/// metadata is never trusted for offset, content identity or output limits.
#[derive(Default)]
struct LogCursor{
    next:usize,
    prefix_sha256:Option<String>,
}
fn next_log_chunk(
    root:&crate::security::paths::Root,
    name:&str,
    cursor:&mut LogCursor,
    max_chunk:usize,
    max_total:usize,
)->Result<Vec<u8>>{
    if !matches!(name,"child-stdout.log"|"child-stderr.log"){
        bail!("Only exact AppContainer stdout/stderr log names are accepted");
    }
    if max_chunk==0||max_chunk>32*1024||max_total==0||max_total>MAX_STAGED_FILE{
        bail!("Invalid log chunk or total byte budget");
    }
    let Some(content)=root.read_optional(name,max_total)?else{
        if cursor.next>0{bail!("AppContainer log disappeared after streaming began");}
        return Ok(Vec::new());
    };
    if cursor.next>content.len(){
        bail!("AppContainer log shrank after streaming began");
    }
    if let Some(expected)=&cursor.prefix_sha256{
        if crate::util::digest(&content[..cursor.next])!=*expected{
            bail!("AppContainer log prefix was changed after streaming");
        }
    }
    let end=cursor.next.saturating_add(max_chunk).min(content.len());
    let part=content[cursor.next..end].to_vec();
    cursor.next=end;
    cursor.prefix_sha256=Some(crate::util::digest(&content[..end]));
    Ok(part)
}
// A process may write only a bounded pair of *private* files. This
// reader owns the cursors; an untrusted child cannot provide offsets or
// overwrite already-consumed prefixes without aborting the entire job.
pub const MAX_PRODUCTION_LOG_BYTES:usize=64*1024;
const PRODUCTION_LOG_PAGE_BYTES:usize=4096;

struct BoundedLogBroker{
    root:crate::security::paths::Root,
    stdout:LogCursor,
    stderr:LogCursor,
    stdout_data:Vec<u8>,
    stderr_data:Vec<u8>,
    budget:usize,
}
impl BoundedLogBroker{
    fn open(output_dir:&Path,budget:usize)->Result<Self>{
        if budget==0||budget>MAX_PRODUCTION_LOG_BYTES{
            bail!("Sandbox log budget must be 1..64 KiB");
        }
        Ok(Self{
            root:crate::security::paths::Root::open(output_dir)?,
            stdout:LogCursor::default(),
            stderr:LogCursor::default(),
            stdout_data:Vec::new(),
            stderr_data:Vec::new(),
            budget,
        })
    }
    fn poll(&mut self)->Result<()>{
        let mut combined=self.stdout_data.len().saturating_add(self.stderr_data.len());
        for (name,cursor,data) in [
            ("child-stdout.log",&mut self.stdout,&mut self.stdout_data),
            ("child-stderr.log",&mut self.stderr,&mut self.stderr_data),
        ]{
            loop{
                let chunk=next_log_chunk(
                    &self.root,name,cursor,PRODUCTION_LOG_PAGE_BYTES,self.budget
                ).with_context(||format!("Read bounded AppContainer {name}"))?;
                if chunk.is_empty(){break;}
                if data.len().saturating_add(chunk.len())>self.budget
                    ||combined.saturating_add(chunk.len())>self.budget{
                    bail!("Combined AppContainer stdout/stderr exceeds byte budget");
                }
                combined+=chunk.len();
                data.extend_from_slice(&chunk);
            }
        }
        Ok(())
    }
    fn finish(&mut self)->Result<serde_json::Value>{
        self.poll()?;
        // Final pinned re-read detects replacements, truncation and missing
        // files, including changes after the last streaming poll.
        let out=self.root.read("child-stdout.log",self.budget)?;
        let err=self.root.read("child-stderr.log",self.budget)?;
        if out!=self.stdout_data||err!=self.stderr_data{
            bail!("AppContainer logs changed after streaming");
        }
        Ok(serde_json::json!({
            "stdout_bytes":out.len(),
            "stdout_sha256":crate::util::digest(&out),
            "stderr_bytes":err.len(),
            "stderr_sha256":crate::util::digest(&err),
            "limit_bytes":self.budget,
            "bounded":true,
            "capture":"private_files_without_parent_handle_inheritance",
        }))
    }
}

#[cfg(test)]
mod tests{
    use super::*;

    // Diagnostics stay inside the synthetic profile and retain the same
    // pinned-path checks. Oversized files report their length, not contents.
    fn stdio_probe_evidence(directory:&Path)->String{
        let root=match crate::security::paths::Root::open(directory){
            Ok(root)=>root,
            Err(error)=>return crate::util::bounded_text(&format!("diagnostics unavailable: {error:#}"),256),
        };
        let mut evidence=Vec::new();
        for name in ["stream-observed.ack","child-stdout.log","child-stderr.log"]{
            let length=fs::symlink_metadata(directory.join(name)).ok().map(|m|m.len());
            let detail=match root.read_optional(name,1024){
                Ok(Some(bytes))=>format!("{:?}",String::from_utf8_lossy(&bytes)),
                Ok(None)=>"missing".into(),
                Err(error)=>format!("{error:#}"),
            };
            evidence.push(format!("{name}: bytes={length:?}, {}",
                crate::util::bounded_text(&detail,384)));
        }
        evidence.join("; ")
    }

    fn wait_for_stdio_probe_ready(
        root:&crate::security::paths::Root,
        finished:&std::sync::atomic::AtomicBool,
    )->Result<()>{
        use std::sync::atomic::Ordering;
        let until=std::time::Instant::now()+std::time::Duration::from_secs(10);
        loop{
            if let Some(ready)=root.read_optional("stream-observed.ack",16)?{
                if ready==b"0"{return Ok(());}
                // create_new publishes the empty file before write_all publishes readiness.
                if !ready.is_empty(){
                    bail!("Unexpected AppContainer stdio readiness marker");
                }
            }
            if finished.load(Ordering::SeqCst){
                bail!("AppContainer probe exited before publishing stdio readiness");
            }
            if std::time::Instant::now()>=until{
                bail!("AppContainer probe did not publish stdio readiness within 10 seconds");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn protocol_hash_worker_runs_with_verified_appcontainer_token(){
        if std::env::var("ENDLESSVIBE_PROTOCOL_WORKER").ok().as_deref()!=Some("1"){
            return;
        }
        run_fixed_worker_from_env().unwrap();
    }

    #[test]
    fn fixed_worker_refuses_non_appcontainer_process(){
        assert!(!has_appcontainer_token(unsafe{GetCurrentProcess()}).unwrap());
        assert!(run_fixed_worker_from_env().is_err());
    }

    #[test]
    fn appcontainer_protocol_broker_verifies_end_to_end_hash_and_identity(){
        use crate::tools::sandbox_protocol::{WorkerReceipt,WorkerRequest,RECEIPT_NAME};
        let profile=Profile::create().unwrap();
        let project=tempfile::tempdir().unwrap();
        fs::write(project.path().join("data.rs"),b"fn sandboxed() {}\n").unwrap();
        fs::write(project.path().join(".env"),b"SHOULD_NEVER_BE_COPIED").unwrap();
        let authorized=crate::security::paths::Root::open(project.path()).unwrap();
        let input=profile.stage_project_file(&authorized,"data.rs").unwrap();
        assert!(input.ends_with("inputs/data.rs"));
        assert!(profile.stage_project_file(&authorized,".env").is_err());
        let results=profile.prepare_results().unwrap();
        let private=profile.folder().unwrap().join("LocalState/EndlessVibe");
        let request=WorkerRequest::new("test_job_01","inputs/data.rs").unwrap();
        let request_file=private.join("worker-request.json");
        crate::util::private_create(&request_file,&request.encode().unwrap()).unwrap();
        let exe=private.join("protocol-probe.exe");
        fs::copy(std::env::current_exe().unwrap(),&exe).unwrap();
        let run="tools::windows_appcontainer::tests::protocol_hash_worker_runs_with_verified_appcontainer_token";
        let status=launch_isolated_test(&profile,&exe,&format!("--exact {run} --nocapture"),
            &[
                ("ENDLESSVIBE_PROTOCOL_WORKER","1".into()),
                ("ENDLESSVIBE_PROTOCOL_ROOT",private.to_string_lossy().into_owned()),
            ]).unwrap();
        assert_eq!(status,0,"Verified AppContainer worker did not complete");
        let output=profile.collect_result(RECEIPT_NAME,4096).unwrap();
        let receipt=WorkerReceipt::decode(&output.bytes).unwrap();
        let independently_opened=authorized.read("data.rs",
            crate::tools::sandbox_protocol::MAX_INPUT_BYTES).unwrap();
        receipt.verify_against(&request,&independently_opened).unwrap();
        assert_eq!(receipt.input_bytes,independently_opened.len());
        assert_eq!(output.sha256,crate::util::digest(&output.bytes));
        assert!(results.join(RECEIPT_NAME).is_file());

        // A hostile or corrupted sandbox receipt must never authorize
        // publication to the original Project.
        let mut forged=receipt.clone();
        forged.sha256="0".repeat(64);
        fs::write(results.join(RECEIPT_NAME),forged.encode().unwrap()).unwrap();
        let forged_bytes=profile.collect_result(RECEIPT_NAME,4096).unwrap();
        let forged_read=WorkerReceipt::decode(&forged_bytes.bytes).unwrap();
        assert!(forged_read.verify_against(&request,&independently_opened).is_err());
        assert_eq!(authorized.read("data.rs",2048).unwrap(),b"fn sandboxed() {}\n");
    }


    #[test]
    fn explicit_profile_finish_is_idempotent_and_cleans_journal(){
        let temp=tempfile::tempdir().unwrap();
        let journal=temp.path().join("pending-profiles");
        let mut profile=Profile::create_journaled(&journal).unwrap();
        assert_eq!(fs::read_dir(&journal).unwrap().count(),1);
        profile.finish().unwrap();
        profile.finish().unwrap();
        drop(profile);
        assert_eq!(fs::read_dir(&journal).unwrap().count(),0);
    }

    #[test]
    fn durable_profile_journal_is_cleared_after_normal_cleanup(){
        let temp=tempfile::tempdir().unwrap();
        let journal=temp.path().join("pending-profiles");
        let profile=Profile::create_journaled(&journal).unwrap();
        assert_eq!(fs::read_dir(&journal).unwrap().count(),1);
        drop(profile);
        assert_eq!(fs::read_dir(&journal).unwrap().count(),0);
        assert_eq!(recover_orphan_profiles(&journal).unwrap(),0);
    }

    #[test]
    fn journaled_appcontainer_recovers_a_simulated_interrupted_service(){
        let temp=tempfile::tempdir().unwrap();
        let journal=temp.path().join("pending-profiles");
        let profile=Profile::create_journaled(&journal).unwrap();
        assert_eq!(fs::read_dir(&journal).unwrap().count(),1);
        // Simulate a force-terminated service without running Drop. No
        // sandboxed child is active when recovery starts.
        std::mem::forget(profile);
        assert_eq!(recover_orphan_profiles(&journal).unwrap(),1);
        assert_eq!(recover_orphan_profiles(&journal).unwrap(),0);
        assert_eq!(fs::read_dir(&journal).unwrap().count(),0);
    }

    #[test]
    fn recovery_does_not_trust_unrecognized_or_tampered_journal_names(){
        let temp=tempfile::tempdir().unwrap();
        let journal=temp.path().join("pending-profiles");
        crate::util::private_dir(&journal).unwrap();
        let spoofed=journal.join("UnrelatedPackage.pending");
        fs::write(&spoofed,b"spoofed").unwrap();
        assert!(recover_orphan_profiles(&journal).is_err());
        assert!(spoofed.exists(),"Refused records must not be silently discarded");
        fs::remove_file(&spoofed).unwrap();
        let wrong_name=journal.join(format!("EndlessVibe.Isolated.{}.pending","a".repeat(32)));
        fs::write(&wrong_name,b"v1:an-unrelated-profile\n").unwrap();
        assert!(recover_orphan_profiles(&journal).is_err());
        assert!(wrong_name.exists());
    }


    #[test]
    fn sandbox_environment_uses_native_system_directories_only(){
        let profile=Profile::create().unwrap();
        let wide=sandbox_environment(&profile,&[]).unwrap();
        let entries=wide.split(|value|*value==0)
            .filter(|value|!value.is_empty())
            .map(|value|String::from_utf16(value).unwrap())
            .collect::<Vec<_>>();
        let lookup=|name:&str|{
            entries.iter().filter_map(|entry|entry.split_once('='))
                .find(|(key,_)|*key==name).map(|(_,value)|value.to_owned())
        };
        let system32=crate::tools::windows_trusted_worker::system32_directory().unwrap();
        assert_eq!(lookup("PATH").unwrap(),system32.display().to_string());
        let expected_root=system32.parent().unwrap().display().to_string();
        assert_eq!(lookup("SystemRoot").unwrap(),expected_root);
        assert_eq!(lookup("WINDIR").unwrap(),expected_root);
        assert!(lookup("SystemDrive").unwrap().ends_with(':'));
        assert!(lookup("ComSpec").is_none(),"Restricted Worker must not advertise a shell");
        assert!(lookup("PATHEXT").is_none());
        assert_eq!(entries.len(),9,"Only reviewed native/private paths may be inherited");
        assert!(sandbox_environment(&profile,&[
            ("ENDLESSVIBE_LOADER_GATE","1".into())
        ]).is_ok());
        for name in ["ComSpec","PATH","Powershell","SystemRoot","TEMP","ENDLESSVIBE_BAD=KEY"]{
            assert!(sandbox_environment(&profile,&[(name,"x".into())]).is_err(),
                "Unreviewed lowbox environment key {name} was accepted");
        }
    }

    #[test]
    fn sandbox_worker_environment_cannot_override_protected_values(){
        let profile=Profile::create().unwrap();
        assert!(sandbox_environment(&profile,&[]).is_ok());
        assert!(sandbox_environment(&profile,&[
            ("ENDLESSVIBE_PROTOCOL_ROOT",profile.folder().unwrap().display().to_string())
        ]).is_ok());
        for name in ["PATH","path","SystemRoot","TEMP","ComSpec","APPDATA"]{
            assert!(sandbox_environment(&profile,&[(name,"bad".into())]).is_err(),
                "Protected Windows environment variable {name} was overridable");
        }
        for name in ["", "=C:", "BAD-NAME", "a b", "x=y"]{
            assert!(sandbox_environment(&profile,&[(name,"invalid".into())]).is_err());
        }
        assert!(sandbox_environment(&profile,&[
            ("ENDLESSVIBE_TEST_KEY","first".into()),("ENDLESSVIBE_TEST_KEY","second".into())
        ]).is_err(),"Case-insensitive duplicate environment variable accepted");
        assert!(sandbox_environment(&profile,&[
            ("ENDLESSVIBE_TEST_KEY","x".repeat(4097))
        ]).is_err());
    }

    #[test]
    fn production_log_broker_checks_combined_budget_and_replacement(){
        let dir=tempfile::tempdir().unwrap();
        let stdout=dir.path().join("child-stdout.log");
        let stderr=dir.path().join("child-stderr.log");
        assert!(BoundedLogBroker::open(dir.path(),0).is_err());
        assert!(BoundedLogBroker::open(dir.path(),MAX_PRODUCTION_LOG_BYTES+1).is_err());
        let mut broker=BoundedLogBroker::open(dir.path(),8).unwrap();
        fs::write(&stdout,b"hello").unwrap();
        fs::write(&stderr,b"!").unwrap();
        broker.poll().unwrap();
        let report=broker.finish().unwrap();
        assert_eq!(report["stdout_bytes"],5);
        assert_eq!(report["stderr_bytes"],1);
        assert_eq!(report["stdout_sha256"],crate::util::digest(b"hello"));
        assert_eq!(report["stderr_sha256"],crate::util::digest(b"!"));
        fs::write(&stdout,b"HELLO").unwrap();
        assert!(broker.poll().is_err(),
            "Rewriting previously consumed stdout must be refused");

        let mut combined=BoundedLogBroker::open(dir.path(),8).unwrap();
        fs::write(&stdout,b"abcde").unwrap();
        fs::write(&stderr,b"abcd").unwrap();
        assert!(combined.poll().is_err(),
            "Combined stdout/stderr must never exceed the total byte budget");

        let mut missing=BoundedLogBroker::open(dir.path(),8).unwrap();
        fs::remove_file(&stderr).unwrap();
        assert!(missing.finish().is_err(),
            "A missing final stderr stream must fail closed");
    }

    #[test]
    fn production_log_budget_kills_running_appcontainer_job(){
        let profile=Profile::create().unwrap();
        let outputs=profile.prepare_results().unwrap();
        let private=profile.folder().unwrap().join("LocalState/EndlessVibe");
        fs::create_dir_all(&private).unwrap();
        let executable=private.join("budgeted-log-worker.exe");
        fs::copy(std::env::current_exe().unwrap(),&executable).unwrap();
        let stdout=outputs.join("child-stdout.log");
        let stderr=outputs.join("child-stderr.log");
        let ack=outputs.join("stream-observed.ack");
        let mut logs=BoundedLogBroker::open(&outputs,4096).unwrap();
        let worker="tools::windows_appcontainer::tests::isolated_process_writes_separate_stdout_stderr_files";
        let outcome=launch_controlled(
            &profile,&executable,&format!("--exact {worker} --nocapture"),
            &[
                ("ENDLESSVIBE_DIRECT_STDIO_CHILD","1".into()),
                ("ENDLESSVIBE_STDIO_OVERFLOW","1".into()),
                ("ENDLESSVIBE_STDOUT_FILE",stdout.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDERR_FILE",stderr.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDIO_ACK_FILE",ack.to_string_lossy().into_owned()),
            ],
            std::time::Duration::from_secs(7),
            &std::sync::atomic::AtomicBool::new(false),
            Some(&mut logs),
        );
        let evidence=stdio_probe_evidence(&outputs);
        let error=outcome.expect_err(&format!(
            "An overflowing worker must be aborted instead of accepted; {evidence}"));
        let error=format!("{error:#}");
        assert!(error.contains("AppContainer output rejected")
            &&(error.contains("maximum size")||error.contains("exceeds byte budget")),
            "The production broker must reject actual log overflow, not another launch failure: {error}; {evidence}");
        assert!(fs::symlink_metadata(&stdout).is_ok_and(|m|m.is_file()&&m.len()>4096),
            "The fixture must actually produce more than 4096 stdout bytes; {evidence}");
        assert!(profile.collect_result("child-stdout.log",4096).is_err(),
            "Oversized logs must not be brokered as output; {evidence}");
    }

    #[test]
    fn incremental_log_reader_rejects_replacements_and_unbounded_output(){
        let dir=tempfile::tempdir().unwrap();
        let logfile=dir.path().join("child-stdout.log");
        let root=crate::security::paths::Root::open(dir.path()).unwrap();
        let mut cursor=LogCursor::default();
        assert!(next_log_chunk(&root,"child-stdout.log",&mut cursor,3,64).unwrap().is_empty());
        fs::write(&logfile,b"abcdef").unwrap();
        assert_eq!(next_log_chunk(&root,"child-stdout.log",&mut cursor,3,64).unwrap(),b"abc");
        assert_eq!(next_log_chunk(&root,"child-stdout.log",&mut cursor,3,64).unwrap(),b"def");
        assert!(next_log_chunk(&root,"child-stdout.log",&mut cursor,3,64).unwrap().is_empty());
        let mut file=fs::OpenOptions::new().append(true).open(&logfile).unwrap();
        file.write_all(b"ghi").unwrap();drop(file);
        assert_eq!(next_log_chunk(&root,"child-stdout.log",&mut cursor,2,64).unwrap(),b"gh");
        assert_eq!(next_log_chunk(&root,"child-stdout.log",&mut cursor,2,64).unwrap(),b"i");
        assert!(next_log_chunk(&root,"../child-stdout.log",&mut cursor,2,64).is_err());
        assert!(next_log_chunk(&root,".env",&mut cursor,2,64).is_err());
        assert!(next_log_chunk(&root,"child-stdout.log",&mut cursor,0,64).is_err());
        assert!(next_log_chunk(&root,"child-stdout.log",&mut cursor,2,0).is_err());
        assert!(next_log_chunk(&root,"child-stdout.log",&mut cursor,2,MAX_STAGED_FILE+1).is_err());
        fs::write(&logfile,b"xbcdefghi").unwrap();
        assert!(next_log_chunk(&root,"child-stdout.log",&mut cursor,2,64).is_err(),
            "An earlier consumed prefix may not change");
        fs::write(&logfile,b"ab").unwrap();
        assert!(next_log_chunk(&root,"child-stdout.log",&mut cursor,2,64).is_err(),
            "A shrinking log may not be resumed with an old cursor");
        let mut fresh=LogCursor::default();
        fs::write(&logfile,vec![b'Y';70]).unwrap();
        assert!(next_log_chunk(&root,"child-stdout.log",&mut fresh,2,64).is_err(),
            "Log budget must apply to total file size, not just returned chunk");
    }

    #[test]
    fn overflowing_stdout_log_cancels_the_entire_appcontainer_job(){
        use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
        let profile=Profile::create().unwrap();
        let outputs=profile.prepare_results().unwrap();
        let private=profile.folder().unwrap().join("LocalState/EndlessVibe");
        fs::create_dir_all(&private).unwrap();
        let exe=private.join("log-quota-probe.exe");
        fs::copy(std::env::current_exe().unwrap(),&exe).unwrap();
        let stdout=outputs.join("child-stdout.log");
        let stderr=outputs.join("child-stderr.log");
        let ack=outputs.join("stream-observed.ack");
        let cancel=Arc::new(AtomicBool::new(false));
        let monitor_cancel=cancel.clone();
        let finished=Arc::new(AtomicBool::new(false));
        let monitor_finished=finished.clone();
        let observer_dir=outputs.clone();
        let observer=std::thread::spawn(move||->Result<()>{
            let root=crate::security::paths::Root::open(&observer_dir)?;
            wait_for_stdio_probe_ready(&root,&monitor_finished)?;
            let mut cursor=LogCursor::default();
            // Measure the observation window from the child's ready marker,
            // excluding process creation and loader scheduling under load.
            let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
            while std::time::Instant::now()<until{
                match next_log_chunk(&root,"child-stdout.log",&mut cursor,512,4096){
                    Ok(_)=>{},
                    Err(error)=>{
                        if !error.to_string().contains("maximum size"){
                            return Err(error).context("Unexpected log security check failure");
                        }
                        if !fs::symlink_metadata(observer_dir.join("child-stdout.log"))
                            .is_ok_and(|m|m.is_file()&&m.len()>4096){
                            bail!("Log overflow was reported without an oversized stdout file");
                        }
                        monitor_cancel.store(true,Ordering::SeqCst);
                        return Ok(());
                    }
                }
                if monitor_finished.load(Ordering::SeqCst){
                    bail!("AppContainer probe exited before stdout overflow was observed");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            bail!("No stdout overflow within five seconds of child readiness")
        });
        let name="tools::windows_appcontainer::tests::isolated_process_writes_separate_stdout_stderr_files";
        let result=launch_controlled_test(&profile,&exe,&format!("--exact {name} --nocapture"),
            &[
                ("ENDLESSVIBE_DIRECT_STDIO_CHILD","1".into()),
                ("ENDLESSVIBE_STDIO_OVERFLOW","1".into()),
                ("ENDLESSVIBE_STDOUT_FILE",stdout.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDERR_FILE",stderr.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDIO_ACK_FILE",ack.to_string_lossy().into_owned()),
            ],
            std::time::Duration::from_secs(7),&cancel);
        finished.store(true,Ordering::SeqCst);
        let observation=observer.join().unwrap();
        let evidence=stdio_probe_evidence(&outputs);
        let result=result.unwrap_or_else(|error|panic!(
            "Quota probe launch failed: {error:#}; observation={observation:?}; {evidence}"));
        assert!(observation.is_ok(),
            "Log budget violation should be observed; outcome={result:?}; observation={observation:?}; {evidence}");
        assert!(result.cancelled&&!result.timed_out,
            "Confirmed stdout overflow must cancel the Job Object; outcome={result:?}; {evidence}");
        assert!(profile.collect_result("child-stdout.log",4096).is_err(),
            "Oversized logs must never be brokered into the Project; {evidence}");
    }


    // The AppContainer child opens its own private log files and then changes
    // only its *own* standard handles. This shares NO handles with the parent
    // service, avoiding the unsafe cross-token inheritance path.
    #[test]
    fn isolated_process_writes_separate_stdout_stderr_files(){
        if std::env::var("ENDLESSVIBE_DIRECT_STDIO_CHILD").ok().as_deref()!=Some("1"){return;}
        use windows_sys::Win32::System::Console::{
            SetStdHandle,STD_OUTPUT_HANDLE,STD_ERROR_HANDLE,
        };
        use std::os::windows::io::AsRawHandle;
        assert!(has_appcontainer_token(unsafe{GetCurrentProcess()}).unwrap());
        let out_name=std::env::var("ENDLESSVIBE_STDOUT_FILE").unwrap();
        let err_name=std::env::var("ENDLESSVIBE_STDERR_FILE").unwrap();
        let ack_name=std::env::var("ENDLESSVIBE_STDIO_ACK_FILE").unwrap();
        // SetStdHandle stores borrowed handle values. Preserve the owning
        // Files before installing them, including overflow returns and panic
        // unwinding; libtest may still print after this test function exits.
        let out=std::mem::ManuallyDrop::new(
            fs::OpenOptions::new().write(true).create_new(true).open(out_name).unwrap());
        let err=std::mem::ManuallyDrop::new(
            fs::OpenOptions::new().write(true).create_new(true).open(err_name).unwrap());
        assert_ne!(unsafe{SetStdHandle(STD_OUTPUT_HANDLE,out.as_raw_handle() as _)},0);
        assert_ne!(unsafe{SetStdHandle(STD_ERROR_HANDLE,err.as_raw_handle() as _)},0);
        let mut ack=fs::OpenOptions::new().write(true).create_new(true).open(&ack_name).unwrap();
        ack.write_all(b"0").unwrap();
        ack.sync_all().unwrap();
        drop(ack);
        std::io::stdout().write_all(b"APPCONTAINER-STDOUT-EARLY\n").unwrap();
        std::io::stdout().flush().unwrap();
        std::io::stderr().write_all(b"APPCONTAINER-STDERR-EARLY\n").unwrap();
        std::io::stderr().flush().unwrap();
        if std::env::var("ENDLESSVIBE_STDIO_OVERFLOW").ok().as_deref()==Some("1"){
            std::io::stdout().write_all(&vec![b'X';8*1024]).unwrap();
            std::io::stdout().flush().unwrap();
            std::thread::sleep(std::time::Duration::from_secs(9));
            return;
        }
        // A test handshake proves the parent saw the first bytes while the
        // process was still alive, even under parallel test scheduling.
        let until=std::time::Instant::now()+std::time::Duration::from_secs(6);
        while fs::read(&ack_name).unwrap_or_default()!=b"1"{
            assert!(std::time::Instant::now()<until,
                "Parent did not acknowledge the first streamed bytes");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        std::io::stdout().write_all(b"APPCONTAINER-STDOUT-DONE\n").unwrap();
        std::io::stdout().flush().unwrap();
        std::io::stderr().write_all(b"APPCONTAINER-STDERR-DONE\n").unwrap();
        std::io::stderr().flush().unwrap();
    }

    #[test]
    fn appcontainer_direct_stdio_is_readable_while_process_is_running(){
        use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
        let profile=Profile::create().unwrap();
        let outputs=profile.prepare_results().unwrap();
        let private=profile.folder().unwrap().join("LocalState/EndlessVibe");
        fs::create_dir_all(&private).unwrap();
        let exe=private.join("direct-stdio-probe.exe");
        fs::copy(std::env::current_exe().unwrap(),&exe).unwrap();
        let stdout_name=outputs.join("child-stdout.log");
        let stderr_name=outputs.join("child-stderr.log");
        let ack_name=outputs.join("stream-observed.ack");
        let finished=Arc::new(AtomicBool::new(false));
        let watch_finished=finished.clone();
        let watch_dir=outputs.clone();
        let watch_ack=ack_name.clone();
        let watcher=std::thread::spawn(move||->Result<()>{
            let root=crate::security::paths::Root::open(&watch_dir)?;
            wait_for_stdio_probe_ready(&root,&watch_finished)?;
            let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
            let mut cursor=LogCursor::default();
            let mut seen=Vec::new();
            while std::time::Instant::now()<until{
                let chunk=next_log_chunk(&root,"child-stdout.log",&mut cursor,512,4096)?;
                seen.extend_from_slice(&chunk);
                if seen.windows(b"APPCONTAINER-STDOUT-EARLY".len())
                    .any(|bytes|bytes==b"APPCONTAINER-STDOUT-EARLY")
                    && !seen.windows(b"APPCONTAINER-STDOUT-DONE".len())
                       .any(|bytes|bytes==b"APPCONTAINER-STDOUT-DONE"){
                    fs::write(&watch_ack,b"1").context("Acknowledge first AppContainer stdout bytes")?;
                    return Ok(());
                }
                if watch_finished.load(Ordering::SeqCst){
                    bail!("AppContainer probe exited before early stdout was observed");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            bail!("No early stdout within five seconds of child readiness")
        });
        let name="tools::windows_appcontainer::tests::isolated_process_writes_separate_stdout_stderr_files";
        let result=launch_controlled_test(&profile,&exe,&format!("--exact {name} --nocapture"),
            &[
                ("ENDLESSVIBE_DIRECT_STDIO_CHILD","1".into()),
                ("ENDLESSVIBE_STDOUT_FILE",stdout_name.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDERR_FILE",stderr_name.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDIO_ACK_FILE",ack_name.to_string_lossy().into_owned()),
            ],
            std::time::Duration::from_secs(10),&AtomicBool::new(false),
        );
        finished.store(true,Ordering::SeqCst);
        let observation=watcher.join().unwrap();
        let evidence=stdio_probe_evidence(&outputs);
        let result=result.unwrap_or_else(|error|panic!(
            "Direct stdio probe launch failed: {error:#}; observation={observation:?}; {evidence}"));
        assert!(!result.cancelled&&!result.timed_out&&result.exit_code==0,
            "AppContainer redirected standard output probe failed; outcome={result:?}; observation={observation:?}; {evidence}");
        assert!(observation.is_ok(),
            "Parent must observe early stdout before final output; outcome={result:?}; observation={observation:?}; {evidence}");
        let stdout=profile.collect_result("child-stdout.log",2048).unwrap();
        let stderr=profile.collect_result("child-stderr.log",2048).unwrap();
        assert!(String::from_utf8_lossy(&stdout.bytes).contains("APPCONTAINER-STDOUT-EARLY"));
        assert!(String::from_utf8_lossy(&stdout.bytes).contains("APPCONTAINER-STDOUT-DONE"));
        assert!(String::from_utf8_lossy(&stderr.bytes).contains("APPCONTAINER-STDERR-EARLY"));
        assert!(String::from_utf8_lossy(&stderr.bytes).contains("APPCONTAINER-STDERR-DONE"));
        assert_eq!(stdout.sha256,crate::util::digest(&stdout.bytes));
        assert_eq!(stderr.sha256,crate::util::digest(&stderr.bytes));
    }


    #[test]
    fn child_creates_private_reviewable_output(){
        if std::env::var("ENDLESSVIBE_OUTPUT_CHILD").ok().as_deref()!=Some("1"){return;}
        assert!(has_appcontainer_token(unsafe{GetCurrentProcess()}).unwrap());
        let target=std::env::var("ENDLESSVIBE_OUTPUT_TARGET").unwrap();
        fs::write(target,b"generated in AppContainer").unwrap();
    }
    #[test]
    fn output_broker_checks_bounds_and_keeps_original_project_unchanged(){
        let profile=Profile::create().unwrap();
        let outputs=profile.prepare_results().unwrap();
        let project=tempfile::tempdir().unwrap();
        let original=project.path().join("generated.txt");
        fs::write(&original,b"unmodified original").unwrap();
        let private=profile.folder().unwrap().join("LocalState/EndlessVibe");
        fs::create_dir_all(&private).unwrap();
        let exe=private.join("artifact-probe.exe");
        fs::copy(std::env::current_exe().unwrap(),&exe).unwrap();
        let target=outputs.join("generated.txt");
        let test="tools::windows_appcontainer::tests::child_creates_private_reviewable_output";
        let code=launch_isolated_test(&profile,&exe,&format!("--exact {test}"),
            &[("ENDLESSVIBE_OUTPUT_CHILD","1".into()),
              ("ENDLESSVIBE_OUTPUT_TARGET",target.to_string_lossy().into_owned())]).unwrap();
        assert_eq!(code,0,"AppContainer artifact process did not complete");
        let returned=profile.collect_result("generated.txt",1024).unwrap();
        assert_eq!(returned.relative_path,"generated.txt");
        assert_eq!(returned.bytes,b"generated in AppContainer");
        assert_eq!(returned.sha256,crate::util::digest(&returned.bytes));
        assert_eq!(fs::read(&original).unwrap(),b"unmodified original");
        assert!(profile.collect_result("../generated.txt",1024).is_err());
        assert!(profile.collect_result(".env",1024).is_err());
        assert!(profile.collect_result("generated.txt",4).is_err());
        assert!(profile.collect_result("generated.txt",0).is_err());
        assert!(profile.collect_result("generated.txt",MAX_STAGED_FILE+1).is_err());
        fs::write(outputs.join("huge.txt"),vec![b'a';1025]).unwrap();
        assert!(profile.collect_result("huge.txt",1024).is_err());
        let linked=outputs.join("hardlink.txt");
        if fs::hard_link(&target,&linked).is_ok(){
            assert!(profile.collect_result("hardlink.txt",1024).is_err());
        }
    }


    #[test]
    fn child_sleeps_when_isolated(){
        if std::env::var("ENDLESSVIBE_SLOW_CHILD").ok().as_deref()==Some("1"){
            assert!(has_appcontainer_token(unsafe{GetCurrentProcess()}).unwrap());
            std::thread::sleep(std::time::Duration::from_secs(6));
        }
    }
    #[test]
    fn cancel_stops_isolated_job_without_leaving_orphans(){
        use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
        let profile=Profile::create().unwrap();
        let stage=profile.folder().unwrap().join("LocalState/EndlessVibe");
        fs::create_dir_all(&stage).unwrap();
        let binary=stage.join("cancel-probe.exe");
        fs::copy(std::env::current_exe().unwrap(),&binary).unwrap();
        let cancel=Arc::new(AtomicBool::new(false));
        let signal=cancel.clone();
        let notifier=std::thread::spawn(move||{
            std::thread::sleep(std::time::Duration::from_millis(220));
            signal.store(true,Ordering::SeqCst);
        });
        let child="tools::windows_appcontainer::tests::child_sleeps_when_isolated";
        let result=launch_controlled_test(&profile,&binary,&format!("--exact {child}"),
            &[("ENDLESSVIBE_SLOW_CHILD","1".into())],
            std::time::Duration::from_secs(3),&cancel).unwrap();
        notifier.join().unwrap();
        assert!(result.cancelled&&!result.timed_out);
        assert_ne!(result.exit_code,0);
    }
    #[test]
    fn timeout_stops_isolated_job_without_leaving_orphans(){
        use std::sync::atomic::AtomicBool;
        let profile=Profile::create().unwrap();
        let stage=profile.folder().unwrap().join("LocalState/EndlessVibe");
        fs::create_dir_all(&stage).unwrap();
        let binary=stage.join("timeout-probe.exe");
        fs::copy(std::env::current_exe().unwrap(),&binary).unwrap();
        let child="tools::windows_appcontainer::tests::child_sleeps_when_isolated";
        let result=launch_controlled_test(&profile,&binary,&format!("--exact {child}"),
            &[("ENDLESSVIBE_SLOW_CHILD","1".into())],
            std::time::Duration::from_millis(270),&AtomicBool::new(false)).unwrap();
        assert!(!result.cancelled&&result.timed_out);
        assert_ne!(result.exit_code,0);
    }

    #[test]
    fn appcontainer_profile_requests_no_network_capabilities(){
        let profile=Profile::create().unwrap();
        let caps=profile.capabilities();
        assert!(!caps.AppContainerSid.is_null());
        assert!(caps.Capabilities.is_null());
        assert_eq!(caps.CapabilityCount,0);
        assert!(profile.folder().unwrap().is_absolute());
    }
    #[test]
    fn appcontainer_stage_only_copies_authorized_project_files(){
        let project=tempfile::tempdir().unwrap();
        fs::write(project.path().join("allowed.txt"),b"hello from project").unwrap();
        fs::write(project.path().join(".env"),b"SECRET").unwrap();
        let root=crate::security::paths::Root::open(project.path()).unwrap();
        let profile=Profile::create().unwrap();
        let staged=profile.stage_project_file(&root,"allowed.txt").unwrap();
        assert!(staged.starts_with(profile.folder().unwrap()));
        assert_eq!(fs::read(&staged).unwrap(),b"hello from project");
        assert!(profile.stage_project_file(&root,".env").is_err());
        assert!(profile.stage_project_file(&root,"../allowed.txt").is_err());
        assert_eq!(fs::read(project.path().join("allowed.txt")).unwrap(),b"hello from project");
    }
    #[test]
    fn child_process_enforces_appcontainer_file_and_network_boundaries(){
        if std::env::var("ENDLESSVIBE_APP_CONTAINER_CHILD").ok().as_deref()!=Some("1"){return;}
        assert!(has_appcontainer_token(unsafe{GetCurrentProcess()}).unwrap());
        let staged=std::env::var("ENDLESSVIBE_STAGED_INPUT").unwrap();
        assert_eq!(fs::read(staged).unwrap(),b"allowed data");
        let outside=std::env::var("ENDLESSVIBE_SOURCE_INPUT").unwrap();
        assert!(fs::read(outside).is_err(),"AppContainer must not read original Project path");
        let localhost="127.0.0.1:20001".parse().unwrap();
        assert!(std::net::TcpStream::connect_timeout(
            &localhost,std::time::Duration::from_millis(500),
        ).is_err(),"AppContainer with no capabilities must not access Dashboard loopback");
    }
    #[test]
    fn isolated_child_reads_staged_input_but_not_original_project(){
        let profile=Profile::create().unwrap();
        let project=tempfile::tempdir().unwrap();
        let original=project.path().join("allowed.txt");
        fs::write(&original,b"allowed data").unwrap();
        let root=crate::security::paths::Root::open(project.path()).unwrap();
        let staged=profile.stage_project_file(&root,"allowed.txt").unwrap();
        // Copy the *test program*, not any Project executable, to the private
        // AppContainer profile. It cannot run from the ungranted Project tree.
        let dir=profile.folder().unwrap().join("LocalState/EndlessVibe");
        fs::create_dir_all(&dir).unwrap();
        let exe=dir.join("probe.exe");
        fs::copy(std::env::current_exe().unwrap(),&exe).unwrap();
        let test_name="tools::windows_appcontainer::tests::child_process_enforces_appcontainer_file_and_network_boundaries";
        let args=format!("--exact {test_name} --nocapture");
        let code=launch_isolated_test(
            &profile,&exe,&args,&[
                ("ENDLESSVIBE_APP_CONTAINER_CHILD","1".into()),
                ("ENDLESSVIBE_STAGED_INPUT",staged.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_SOURCE_INPUT",original.to_string_lossy().into_owned()),
            ]
        ).unwrap();
        assert_eq!(code,0,"AppContainer staged-input probe failed");
    }
}
