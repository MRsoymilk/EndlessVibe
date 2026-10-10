//! Native Windows AppContainer process identity and fail-closed launch probe.
//!
//! AppContainer is *not* a synonym for Job Objects: it reduces the process
//! security token and defaults to no network capabilities. Real Project input
//! must be made available in an isolated staging location, not by broadening
//! the original Project ACL. This module is deliberately not a live backend yet.
use anyhow::{bail,Context,Result};
use std::{
    ffi::c_void,
    mem::{size_of,zeroed},
    os::windows::io::{AsRawHandle,FromRawHandle,OwnedHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0,WAIT_TIMEOUT},
    Security::{
        FreeSid,GetTokenInformation,SECURITY_CAPABILITIES,PSID,TokenIsAppContainer,TOKEN_QUERY,
        Isolation::{CreateAppContainerProfile,DeleteAppContainerProfile},
    },
    System::Threading::{
        CreateProcessW,DeleteProcThreadAttributeList,GetExitCodeProcess,
        InitializeProcThreadAttributeList,OpenProcessToken,ResumeThread,
        TerminateProcess,UpdateProcThreadAttribute,WaitForSingleObject,
        CREATE_SUSPENDED,EXTENDED_STARTUPINFO_PRESENT,PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        PROCESS_INFORMATION,STARTUPINFOEXW,
    },
};

fn wide(text:&str)->Vec<u16>{
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(super) struct Profile{
    name:Vec<u16>,
    sid:PSID,
}
impl Profile{
    /// Use a fresh, unguessable profile ID so no other job shares its SID.
    /// Do not reuse another application's profile after an ID collision.
    pub(super) fn create()->Result<Self>{
        let secret=crate::util::random_secret()?;
        let name=format!("EndlessVibe.Isolated.{}",&crate::util::digest(secret)[..32]);
        let wide_name=wide(&name);
        let display=wide("EndlessVibe isolated process");
        let description=wide("Ephemeral untrusted workload with no network capabilities");
        let mut sid:PSID=ptr::null_mut();
        let result=unsafe{CreateAppContainerProfile(
            wide_name.as_ptr(),display.as_ptr(),description.as_ptr(),
            ptr::null(),0,&mut sid,
        )};
        if result<0{
            bail!("CreateAppContainerProfile failed (HRESULT 0x{:08x}); not falling back to host",result as u32);
        }
        if sid.is_null(){
            let _=unsafe{DeleteAppContainerProfile(wide_name.as_ptr())};
            bail!("CreateAppContainerProfile succeeded without an AppContainer SID");
        }
        Ok(Self{name:wide_name,sid})
    }
    pub(super) fn capabilities(&self)->SECURITY_CAPABILITIES{
        SECURITY_CAPABILITIES{
            AppContainerSid:self.sid,
            Capabilities:ptr::null_mut(),CapabilityCount:0,Reserved:0,
        }
    }
}
impl Drop for Profile{
    fn drop(&mut self){
        if !self.sid.is_null(){unsafe{FreeSid(self.sid)};self.sid=ptr::null_mut();}
        // The profile belongs to this unique job; do not delete any existing
        // unrelated profile or a profile that is still hosting a process.
        let _=unsafe{DeleteAppContainerProfile(self.name.as_ptr())};
    }
}

struct Attributes{
    storage:Vec<usize>,
}
impl Attributes{
    fn create(capabilities:&SECURITY_CAPABILITIES)->Result<Self>{
        let mut bytes=0usize;
        unsafe{InitializeProcThreadAttributeList(ptr::null_mut(),1,0,&mut bytes)};
        if bytes==0{bail!("Windows did not provide the AppContainer attribute list size");}
        let slots=bytes.div_ceil(size_of::<usize>());
        let mut storage=vec![0usize;slots];
        let attrs=storage.as_mut_ptr() as _;
        if unsafe{InitializeProcThreadAttributeList(attrs,1,0,&mut bytes)}==0{
            return Err(std::io::Error::last_os_error()).context("Initialize AppContainer attribute list");
        }
        let result=unsafe{UpdateProcThreadAttribute(
            attrs,0,PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            capabilities as *const _ as _,size_of::<SECURITY_CAPABILITIES>(),
            ptr::null_mut(),ptr::null(),
        )};
        if result==0{
            unsafe{DeleteProcThreadAttributeList(attrs)};
            return Err(std::io::Error::last_os_error()).context("Apply AppContainer security capabilities");
        }
        Ok(Self{storage})
    }
    fn ptr(&mut self)->*mut c_void{self.storage.as_mut_ptr() as *mut c_void}
}
impl Drop for Attributes{
    fn drop(&mut self){
        unsafe{DeleteProcThreadAttributeList(self.storage.as_mut_ptr() as _)};
    }
}

struct ProbeChild{
    process:OwnedHandle,
    thread:OwnedHandle,
}
impl ProbeChild{
    fn is_appcontainer(&self)->Result<bool>{
        let mut raw=ptr::null_mut();
        let ok=unsafe{OpenProcessToken(self.process.as_raw_handle() as _,TOKEN_QUERY,&mut raw)};
        if ok==0{return Err(std::io::Error::last_os_error()).context("Open isolated process token");}
        let token=unsafe{OwnedHandle::from_raw_handle(raw as _)};
        let mut flag=0u32;
        let mut returned=0u32;
        let ok=unsafe{GetTokenInformation(
            token.as_raw_handle() as _,TokenIsAppContainer,
            &mut flag as *mut _ as _,size_of::<u32>() as u32,&mut returned,
        )};
        if ok==0{return Err(std::io::Error::last_os_error()).context("Inspect AppContainer token");}
        Ok(flag!=0)
    }
    fn resume(&self)->Result<()>{
        if unsafe{ResumeThread(self.thread.as_raw_handle() as _)}==u32::MAX{
            return Err(std::io::Error::last_os_error()).context("Resume AppContainer probe");
        }
        Ok(())
    }
    fn wait(&self)->Result<u32>{
        match unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,10_000)}{
            WAIT_OBJECT_0=>{
                let mut code=0u32;
                if unsafe{GetExitCodeProcess(self.process.as_raw_handle() as _,&mut code)}==0{
                    return Err(std::io::Error::last_os_error()).context("Read AppContainer probe exit status");
                }
                Ok(code)
            }
            WAIT_TIMEOUT=>bail!("AppContainer probe exceeded ten seconds"),
            _=>Err(std::io::Error::last_os_error()).context("Wait for AppContainer probe"),
        }
    }
}
impl Drop for ProbeChild{
    fn drop(&mut self){
        if unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,0)}==WAIT_TIMEOUT{
            let _=unsafe{TerminateProcess(self.process.as_raw_handle() as _,1)};
            let _=unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,5_000)};
        }
    }
}

/// Test-only proof that Windows actually created an AppContainer token.
/// File permissions to a Project are NOT broadened here.
#[cfg(test)]
fn launch_system_probe(profile:&Profile,program:&Path)->Result<(bool,u32)>{
    let caps=profile.capabilities();
    let mut attrs=Attributes::create(&caps)?;
    let program_name=wide(&program.to_string_lossy());
    let directory=program.parent().context("AppContainer probe needs a system directory")?;
    let current_directory=wide(&directory.to_string_lossy());
    let mut command=wide(&format!("\"{}\" /D /C exit 0",program.display()));
    let mut startup:STARTUPINFOEXW=unsafe{zeroed()};
    startup.StartupInfo.cb=size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList=attrs.ptr() as _;
    let mut process:PROCESS_INFORMATION=unsafe{zeroed()};
    let ok=unsafe{CreateProcessW(
        program_name.as_ptr(),command.as_mut_ptr(),
        ptr::null(),ptr::null(),0,
        CREATE_SUSPENDED|EXTENDED_STARTUPINFO_PRESENT,
        ptr::null(),current_directory.as_ptr(),
        &startup.StartupInfo,&mut process,
    )};
    if ok==0{return Err(std::io::Error::last_os_error()).context("Launch AppContainer probe");}
    let child=ProbeChild{
        process:unsafe{OwnedHandle::from_raw_handle(process.hProcess as _)},
        thread:unsafe{OwnedHandle::from_raw_handle(process.hThread as _)},
    };
    let isolated=child.is_appcontainer()?;
    if !isolated{bail!("AppContainer child did not receive an AppContainer access token");}
    child.resume()?;
    Ok((isolated,child.wait()?))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn appcontainer_profile_requests_no_network_capabilities(){
        let profile=Profile::create().unwrap();
        let caps=profile.capabilities();
        assert!(!caps.AppContainerSid.is_null());
        assert!(caps.Capabilities.is_null());
        assert_eq!(caps.CapabilityCount,0);
    }
    #[test]
    fn windows_process_uses_actual_appcontainer_access_token(){
        let profile=Profile::create().unwrap();
        let root=std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(||std::path::PathBuf::from(r"C:\Windows"));
        let path=root.join("System32/cmd.exe");
        let (isolated,_exit)=launch_system_probe(&profile,&path).unwrap();
        assert!(isolated);
    }
}
