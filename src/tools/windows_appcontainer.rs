//! Windows AppContainer identity, safe Project input staging, and native probes.
//!
//! The launch path remains test-only until an audited command runner can
//! forward bounded stdout/stderr and handle cancellation. AppContainer denies
//! network capabilities by default; copies in the per-job profile avoid
//! persistent ACL changes to the original Project.
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
    pub(super) fn create()->Result<Self>{
        let random=crate::util::random_secret()?;
        let name=format!("EndlessVibe.Isolated.{}",&crate::util::digest(random)[..32]);
        Self::create_named(&name,None)
    }

    /// Persist the profile identity *before* creating it so a service crash
    /// never leaves an untracked lowbox profile registered in Windows.
    #[cfg(test)]
    fn create_journaled(journal_dir:&Path)->Result<Self>{
        crate::util::private_dir(journal_dir)?;
        let random=crate::util::random_secret()?;
        let name=format!("EndlessVibe.Isolated.{}",&crate::util::digest(random)[..32]);
        let marker=journal_dir.join(format!("{name}.pending"));
        crate::util::private_create(&marker,format!("v1:{name}\n").as_bytes())?;
        match Self::create_named(&name,Some(marker.clone())){
            Ok(profile)=>Ok(profile),
            Err(error)=>{
                let _=fs::remove_file(&marker);
                Err(error)
            }
        }
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
impl Drop for Profile{
    fn drop(&mut self){
        if !self.sid.is_null(){unsafe{FreeSid(self.sid)};self.sid=ptr::null_mut();}
        let result=unsafe{DeleteAppContainerProfile(self.name.as_ptr())};
        if result>=0{
            if let Some(path)=&self.journal{let _=fs::remove_file(path);}
        }
    }
}

// Only call this when the service's single-instance lock has been acquired and
// no old command job can still be alive. The production service does not yet
// invoke this test-only recovery path.
#[cfg(test)]
fn recover_orphan_profiles(journal_dir:&Path)->Result<usize>{
    crate::util::private_dir(journal_dir)?;
    let root=crate::security::paths::Root::open(journal_dir)?;
    let entries=root.entries(".")?;
    if entries.len()>128{bail!("Too many pending AppContainer recovery records");}
    let mut deleted=0usize;
    for entry in entries{
        if entry.kind!="file"{bail!("AppContainer recovery records must be regular files");}
        let name=entry.name.strip_suffix(".pending")
            .context("Unexpected AppContainer recovery record name")?;
        let suffix=name.strip_prefix("EndlessVibe.Isolated.")
            .context("Recovery refuses profiles outside the EndlessVibe namespace")?;
        if suffix.len()!=32||!suffix.bytes().all(|b|b.is_ascii_hexdigit()&&!b.is_ascii_uppercase()){
            bail!("Invalid AppContainer recovery record identifier");
        }
        let bytes=root.read(&entry.name,128)?;
        if bytes!=format!("v1:{name}\n").as_bytes(){
            bail!("AppContainer recovery record content mismatches its filename");
        }
        let wide_name=wide(name);
        let hr=unsafe{DeleteAppContainerProfile(wide_name.as_ptr())};
        // A crash could occur after writing the journal but before profile
        // creation. ERROR_NOT_FOUND is a safe already-absent result.
        if hr<0 && hr as u32!=0x80070490{
            bail!("DeleteAppContainerProfile recovery failed: HRESULT 0x{:08x}",hr as u32);
        }
        fs::remove_file(journal_dir.join(&entry.name))?;
        deleted+=1;
    }
    Ok(deleted)
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

#[cfg(test)]
fn sandbox_environment(profile:&Profile,extras:&[(&str,String)])->Result<Vec<u16>>{
    let root=profile.folder()?;
    let temp=root.join("Temp");
    fs::create_dir_all(&temp)?;
    let system_root=std::env::var("SystemRoot").unwrap_or_else(|_|r"C:\Windows".into());
    let system_dir=PathBuf::from(&system_root).join("System32");
    let system_drive=std::env::var("SystemDrive").unwrap_or_else(|_|"C:".into());
    let mut values=vec![
        ("SystemRoot".to_owned(),system_root.clone()),
        ("WINDIR".to_owned(),system_root),
        ("SystemDrive".to_owned(),system_drive),
        ("PATH".to_owned(),system_dir.to_string_lossy().into_owned()),
        ("ComSpec".to_owned(),system_dir.join("cmd.exe").to_string_lossy().into_owned()),
        ("LOCALAPPDATA".to_owned(),root.to_string_lossy().into_owned()),
        ("APPDATA".to_owned(),root.to_string_lossy().into_owned()),
        ("USERPROFILE".to_owned(),root.to_string_lossy().into_owned()),
        ("TEMP".to_owned(),temp.to_string_lossy().into_owned()),
        ("TMP".to_owned(),temp.to_string_lossy().into_owned()),
    ];
    for (key,value) in extras{
        if key.contains(['=','\0'])||value.contains('\0')||key.len()>128{
            bail!("Invalid AppContainer probe environment variable");
        }
        values.push(((*key).to_owned(),value.clone()));
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

#[cfg(test)]
#[derive(Debug)]
struct ProbeOutcome{exit_code:u32,cancelled:bool,timed_out:bool}
#[cfg(test)]
struct ProbeProcess{process:OwnedHandle,thread:OwnedHandle}
#[cfg(test)]
impl ProbeProcess{
    fn resume_and_wait(
        &self,job:&crate::tools::windows_job::JobObject,
        timeout:std::time::Duration,cancel:&std::sync::atomic::AtomicBool,
    )->Result<ProbeOutcome>{
        use std::sync::atomic::Ordering;
        if unsafe{ResumeThread(self.thread.as_raw_handle() as _)}==u32::MAX{
            return Err(std::io::Error::last_os_error()).context("Resume isolated probe");
        }
        let deadline=std::time::Instant::now()+timeout;
        let(mut cancelled,mut timed_out)=(false,false);
        loop{
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
        let mut code=0u32;
        if unsafe{GetExitCodeProcess(self.process.as_raw_handle() as _,&mut code)}==0{
            return Err(std::io::Error::last_os_error()).context("Read isolated process exit code");
        }
        Ok(ProbeOutcome{exit_code:code,cancelled,timed_out})
    }
}
#[cfg(test)]
impl Drop for ProbeProcess{
    fn drop(&mut self){
        if unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,0)}==WAIT_TIMEOUT{
            let _=unsafe{TerminateProcess(self.process.as_raw_handle() as _,1)};
            let _=unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,5_000)};
        }
    }
}

#[cfg(test)]
fn launch_controlled_test(
    profile:&Profile,program:&Path,args:&str,extras:&[(&str,String)],
    timeout:std::time::Duration,cancel:&std::sync::atomic::AtomicBool,
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
    assert!(job.contains(child.dwProcessId)?,"AppContainer probe escaped its Job Object");
    process.resume_and_wait(&job,timeout,cancel)
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


/// Append-only raw-byte log cursor. The parent owns this state; child-provided
/// metadata is never trusted for offset, content identity or output limits.
#[cfg(test)]
#[derive(Default)]
struct LogCursor{
    next:usize,
    prefix_sha256:Option<String>,
}
#[cfg(test)]
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
#[cfg(test)]
mod tests{
    use super::*;

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
        let observer_dir=outputs.clone();
        let observer=std::thread::spawn(move||{
            let root=crate::security::paths::Root::open(&observer_dir).unwrap();
            let mut cursor=LogCursor::default();
            let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
            while std::time::Instant::now()<until{
                match next_log_chunk(&root,"child-stdout.log",&mut cursor,512,4096){
                    Ok(_)=>{},
                    Err(error)=>{
                        assert!(error.to_string().contains("maximum size"),
                            "Unexpected security check failure: {error}");
                        monitor_cancel.store(true,Ordering::SeqCst);
                        return true;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
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
            std::time::Duration::from_secs(7),&cancel).unwrap();
        assert!(observer.join().unwrap(),"Log budget violation should be observed");
        assert!(result.cancelled&&!result.timed_out);
        assert!(profile.collect_result("child-stdout.log",4096).is_err(),
            "Oversized logs must never be brokered into the Project");
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
        let mut ack=fs::OpenOptions::new().write(true).create_new(true).open(&ack_name).unwrap();
        ack.write_all(b"0").unwrap();
        ack.sync_all().unwrap();
        drop(ack);
        let out=fs::OpenOptions::new().write(true).create_new(true).open(out_name).unwrap();
        let err=fs::OpenOptions::new().write(true).create_new(true).open(err_name).unwrap();
        assert_ne!(unsafe{SetStdHandle(STD_OUTPUT_HANDLE,out.as_raw_handle() as _)},0);
        assert_ne!(unsafe{SetStdHandle(STD_ERROR_HANDLE,err.as_raw_handle() as _)},0);
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
        // The test harness may print after this function returns; preserve
        // the handles until the entire restricted test process terminates.
        std::mem::forget(out);
        std::mem::forget(err);
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
        let observed=Arc::new(AtomicBool::new(false));
        let signal=observed.clone();
        let watch_dir=outputs.clone();
        let watch_ack=ack_name.clone();
        let watcher=std::thread::spawn(move||{
            let root=crate::security::paths::Root::open(&watch_dir).unwrap();
            let until=std::time::Instant::now()+std::time::Duration::from_secs(5);
            let mut cursor=LogCursor::default();
            let mut seen=Vec::new();
            while std::time::Instant::now()<until{
                if let Ok(chunk)=next_log_chunk(&root,"child-stdout.log",&mut cursor,512,4096){
                    seen.extend_from_slice(&chunk);
                    if seen.windows(b"APPCONTAINER-STDOUT-EARLY".len())
                        .any(|bytes|bytes==b"APPCONTAINER-STDOUT-EARLY")
                        && !seen.windows(b"APPCONTAINER-STDOUT-DONE".len())
                           .any(|bytes|bytes==b"APPCONTAINER-STDOUT-DONE"){
                        if fs::write(&watch_ack,b"1").is_ok(){
                            signal.store(true,Ordering::SeqCst);
                            break;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let name="tools::windows_appcontainer::tests::isolated_process_writes_separate_stdout_stderr_files";
        let exit=launch_isolated_test(&profile,&exe,&format!("--exact {name} --nocapture"),
            &[
                ("ENDLESSVIBE_DIRECT_STDIO_CHILD","1".into()),
                ("ENDLESSVIBE_STDOUT_FILE",stdout_name.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDERR_FILE",stderr_name.to_string_lossy().into_owned()),
                ("ENDLESSVIBE_STDIO_ACK_FILE",ack_name.to_string_lossy().into_owned()),
            ]
        ).unwrap();
        watcher.join().unwrap();
        assert_eq!(exit,0,"AppContainer redirected standard output probe failed");
        assert!(observed.load(Ordering::SeqCst),
            "Parent must observe first stdout bytes before the final output is written");
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
