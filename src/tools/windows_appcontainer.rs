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
}
impl Profile{
    pub(super) fn create()->Result<Self>{
        let random=crate::util::random_secret()?;
        let name=format!("EndlessVibe.Isolated.{}",&crate::util::digest(random)[..32]);
        let profile_name=wide(&name);
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
        Ok(Self{name:profile_name,sid})
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
}
impl Drop for Profile{
    fn drop(&mut self){
        if !self.sid.is_null(){unsafe{FreeSid(self.sid)};self.sid=ptr::null_mut();}
        let _=unsafe{DeleteAppContainerProfile(self.name.as_ptr())};
    }
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

#[cfg(test)]
mod tests{
    use super::*;

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
