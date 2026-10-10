//! Windows Job Object lifecycle for spawned command jobs.
//!
//! A process is started suspended, assigned to the Job Object, and only then
//! resumed. This avoids the race where a process could spawn children before
//! limits are active. Job Objects contain process trees and resources; they do
//! NOT restrict filesystem access, registry, network, or Windows user tokens.
use anyhow::{bail,Context,Result};
use std::{mem::{size_of,zeroed},os::windows::{
    io::{AsRawHandle,FromRawHandle,OwnedHandle},
    process::CommandExt,
},ptr};
use tokio::process::{Child,Command};
use windows_sys::Win32::{
    Foundation::{INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot,Thread32First,Thread32Next,THREADENTRY32,TH32CS_SNAPTHREAD,
        },
        JobObjects::{
            AssignProcessToJobObject,CreateJobObjectW,
            JobObjectExtendedLimitInformation,JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS,JOB_OBJECT_LIMIT_JOB_MEMORY,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,SetInformationJobObject,TerminateJobObject,
        },
        Threading::{
            OpenProcess,OpenThread,ResumeThread,CREATE_SUSPENDED,PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SET_QUOTA,PROCESS_TERMINATE,THREAD_SUSPEND_RESUME,
        },
    },
};

#[cfg(test)]use windows_sys::Win32::System::JobObjects::{IsProcessInJob,QueryInformationJobObject};

pub struct JobObject{
    handle:OwnedHandle,
}
impl JobObject {
    pub(super) fn create(memory_mb:u64,processes:u64)->Result<Self>{
        let bytes=memory_mb.checked_mul(1024*1024)
            .and_then(|value|usize::try_from(value).ok())
            .context("Windows Job Object memory limit exceeds architecture capacity")?;
        let count=u32::try_from(processes)
            .context("Windows Job Object process limit exceeds u32")?;
        if bytes==0||count==0{bail!("Windows Job Object limits must be positive");}
        let raw=unsafe{CreateJobObjectW(ptr::null(),ptr::null())};
        if raw.is_null(){
            return Err(std::io::Error::last_os_error()).context("Create Windows Job Object");
        }
        let handle=unsafe{OwnedHandle::from_raw_handle(raw as _)};
        let mut info:JOBOBJECT_EXTENDED_LIMIT_INFORMATION=unsafe{zeroed()};
        info.BasicLimitInformation.LimitFlags=
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE|JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            |JOB_OBJECT_LIMIT_JOB_MEMORY;
        info.BasicLimitInformation.ActiveProcessLimit=count;
        info.JobMemoryLimit=bytes;
        let ok=unsafe{SetInformationJobObject(
            handle.as_raw_handle() as _,JobObjectExtendedLimitInformation,
            &info as *const _ as _,size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )};
        if ok==0{
            return Err(std::io::Error::last_os_error()).context("Configure Windows Job Object resource limits");
        }
        Ok(Self{handle})
    }
    pub(super) fn assign(&self,pid:u32)->Result<()>{
        let process=unsafe{OpenProcess(
            PROCESS_SET_QUOTA|PROCESS_TERMINATE|PROCESS_QUERY_LIMITED_INFORMATION,0,pid,
        )};
        if process.is_null(){
            return Err(std::io::Error::last_os_error()).context("Open suspended Windows child");
        }
        let process=unsafe{OwnedHandle::from_raw_handle(process as _)};
        if unsafe{AssignProcessToJobObject(self.handle.as_raw_handle() as _,process.as_raw_handle() as _)}==0{
            return Err(std::io::Error::last_os_error()).context(
                "Assign suspended child to Job Object (nested jobs may be restricted)"
            );
        }
        Ok(())
    }
    pub fn terminate(&self){
        // Safe even when the original child already exited; descendants must
        // never survive a completed/cancelled user command.
        let _=unsafe{TerminateJobObject(self.handle.as_raw_handle() as _,1)};
    }
    #[cfg(test)]
    pub(super) fn contains(&self,pid:u32)->Result<bool>{
        let raw=unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,pid)};
        if raw.is_null(){return Err(std::io::Error::last_os_error()).context("Open child for job membership check");}
        let process=unsafe{OwnedHandle::from_raw_handle(raw as _)};
        let mut assigned=0i32;
        if unsafe{IsProcessInJob(process.as_raw_handle() as _,self.handle.as_raw_handle() as _,&mut assigned)}==0{
            return Err(std::io::Error::last_os_error()).context("Verify Windows Job Object membership");
        }
        Ok(assigned!=0)
    }
}

fn resume_suspended_primary_thread(pid:u32)->Result<()>{
    let snapshot=unsafe{CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD,0)};
    if snapshot==INVALID_HANDLE_VALUE{
        return Err(std::io::Error::last_os_error()).context("Enumerate suspended Windows child threads");
    }
    let snapshot=unsafe{OwnedHandle::from_raw_handle(snapshot as _)};
    let mut entry:THREADENTRY32=unsafe{zeroed()};
    entry.dwSize=size_of::<THREADENTRY32>() as u32;
    let mut valid=unsafe{Thread32First(snapshot.as_raw_handle() as _,&mut entry)};
    while valid!=0{
        if entry.th32OwnerProcessID==pid{
            let thread=unsafe{OpenThread(THREAD_SUSPEND_RESUME,0,entry.th32ThreadID)};
            if thread.is_null(){
                return Err(std::io::Error::last_os_error()).context("Open suspended primary thread");
            }
            let thread=unsafe{OwnedHandle::from_raw_handle(thread as _)};
            if unsafe{ResumeThread(thread.as_raw_handle() as _)}==u32::MAX{
                return Err(std::io::Error::last_os_error()).context("Resume Job Object child");
            }
            return Ok(());
        }
        valid=unsafe{Thread32Next(snapshot.as_raw_handle() as _,&mut entry)};
    }
    bail!("Suspended Windows child has no primary thread")
}

/// Start a Windows user command with limits attached before any child code runs.
/// If any step fails, the suspended child is killed rather than escaping limits.
pub fn spawn(
    command:&mut Command,memory_mb:u64,max_processes:u64,
)->Result<(Child,JobObject)>{
    let object=JobObject::create(memory_mb,max_processes)?;
    command.as_std_mut().creation_flags(CREATE_SUSPENDED);
    let mut child=command.spawn().context("Spawn suspended Windows command")?;
    let pid=child.id().context("Suspended Windows command has no PID")?;
    if let Err(error)=object.assign(pid).and_then(|_|resume_suspended_primary_thread(pid)){
        object.terminate();
        let _=child.start_kill();
        return Err(error);
    }
    Ok((child,object))
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::process::Stdio;
    #[test]
    fn invalid_limits_are_rejected(){
        assert!(JobObject::create(0,1).is_err());
        assert!(JobObject::create(128,0).is_err());
        assert!(JobObject::create(u64::MAX,2).is_err());
    }
    #[test]
    fn resource_limits_are_programmed_into_job_object(){
        let job=JobObject::create(128,3).unwrap();
        let mut info:JOBOBJECT_EXTENDED_LIMIT_INFORMATION=unsafe{zeroed()};
        let returned=unsafe{QueryInformationJobObject(
            job.handle.as_raw_handle() as _,JobObjectExtendedLimitInformation,
            &mut info as *mut _ as _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ptr::null_mut(),
        )};
        assert_ne!(returned,0);
        assert_eq!(info.BasicLimitInformation.ActiveProcessLimit,3);
        assert_eq!(info.JobMemoryLimit,128*1024*1024);
        let flags=info.BasicLimitInformation.LimitFlags;
        assert_ne!(flags&JOB_OBJECT_LIMIT_JOB_MEMORY,0);
        assert_ne!(flags&JOB_OBJECT_LIMIT_ACTIVE_PROCESS,0);
        assert_ne!(flags&JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,0);
    }
    #[tokio::test]
    async fn suspended_child_is_attached_and_resumed(){
        let mut command=Command::new("cmd.exe");
        command.args(["/D","/C","exit 0"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .kill_on_drop(true);
        let (mut child,job)=spawn(&mut command,256,8).unwrap();
        let pid=child.id().unwrap();
        assert!(job.contains(pid).unwrap());
        let result=tokio::time::timeout(std::time::Duration::from_secs(10),child.wait()).await.unwrap().unwrap();
        assert!(result.success());
        job.terminate();
    }
}
