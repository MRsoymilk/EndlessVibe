//! Native, offline Windows command execution in a Less Privileged AppContainer.
//!
//! The caller owns the project/toolchain ACL lease. Keep it and `Identity`
//! alive until `run` has confirmed that the complete Job Object is empty.
//! This runner never falls back to an ordinary host token.
use anyhow::{bail,Context,Result};
use std::{
    collections::BTreeMap,
    ffi::{c_void,OsStr,OsString},
    fs,
    io::Read,
    mem::{size_of,zeroed},
    os::windows::{ffi::{OsStrExt,OsStringExt},io::{AsRawHandle,FromRawHandle,OwnedHandle}},
    path::{Path,PathBuf},
    ptr,
    sync::{Arc,atomic::{AtomicBool,Ordering}},
    time::{Duration,Instant},
};
use tokio_util::sync::CancellationToken;
use super::windows_execution_recovery::Record;
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE,ERROR_INSUFFICIENT_BUFFER,ERROR_NO_DATA,
        ERROR_PIPE_NOT_CONNECTED,HANDLE,HANDLE_FLAG_INHERIT,LocalFree,
        SetHandleInformation,WAIT_OBJECT_0,WAIT_TIMEOUT,
    },
    Security::{
        AccessCheck,DeriveCapabilitySidsFromName,DuplicateToken,EqualSid,FreeSid,GENERIC_MAPPING,
        GetLengthSid,GetSidSubAuthority,GetSidSubAuthorityCount,
        GetTokenInformation,IsValidSid,PRIVILEGE_SET,PSID,SECURITY_ATTRIBUTES,
        SECURITY_CAPABILITIES,SecurityImpersonation,SID_AND_ATTRIBUTES,TOKEN_APPCONTAINER_INFORMATION,
        TOKEN_DUPLICATE,TOKEN_GROUPS,TOKEN_MANDATORY_LABEL,TOKEN_QUERY,
        TokenAppContainerSid,TokenCapabilities,TokenIntegrityLevel,TokenIsAppContainer,
        Authorization::{
            ConvertSidToStringSidW,ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        Isolation::{CreateAppContainerProfile,DeleteAppContainerProfile,GetAppContainerFolderPath},
    },
    Storage::FileSystem::{FILE_ALL_ACCESS,FILE_GENERIC_EXECUTE,FILE_GENERIC_READ,FILE_GENERIC_WRITE},
    System::{
        Com::CoTaskMemFree,
        JobObjects::{
            CreateJobObjectW,IsProcessInJob,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS,JOB_OBJECT_LIMIT_JOB_MEMORY,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation,QueryInformationJobObject,
            SetInformationJobObject,TerminateJobObject,
        },
        Pipes::{CreatePipe,PeekNamedPipe},
        Threading::{
            CreateProcessW,DeleteProcThreadAttributeList,GetExitCodeProcess,
            InitializeProcThreadAttributeList,OpenProcessToken,ResumeThread,
            TerminateProcess,UpdateProcThreadAttribute,WaitForSingleObject,
            CREATE_NO_WINDOW,CREATE_SUSPENDED,CREATE_UNICODE_ENVIRONMENT,
            EXTENDED_STARTUPINFO_PRESENT,PROCESS_INFORMATION,
            PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,PROC_THREAD_ATTRIBUTE_JOB_LIST,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            STARTF_USESTDHANDLES,STARTUPINFOEXW,
        },
    },
};

const MAX_TOKEN_BYTES:usize=64*1024;
const POLL_INTERVAL:Duration=Duration::from_millis(10);
const CLEANUP_TIMEOUT:Duration=Duration::from_secs(5);
const PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT:u32=1;
// WinNT's SE_GROUP_ENABLED value for the one fixed capability entry.
const CAPABILITY_ENABLED:u32=0x0000_0004;

fn wide(value:&OsStr)->Result<Vec<u16>>{
    let mut text=value.encode_wide().collect::<Vec<_>>();
    if text.contains(&0){bail!("Windows command values cannot contain NUL");}
    text.push(0);
    Ok(text)
}
fn wide_text(value:&str)->Result<Vec<u16>>{wide(OsStr::new(value))}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation{
    fn drop(&mut self){if !self.0.is_null(){unsafe{LocalFree(self.0)};}}
}
struct CapabilitySidArray{pointer:*mut PSID,count:u32}
impl Drop for CapabilitySidArray{
    fn drop(&mut self){
        if !self.pointer.is_null(){
            for index in 0..self.count{
                let sid=unsafe{*self.pointer.add(index as usize)};
                if !sid.is_null(){unsafe{LocalFree(sid)};}
            }
            unsafe{LocalFree(self.pointer as _)};
        }
    }
}
/// A fixed allowlist, not a caller-supplied capability name or configuration.
/// Native Windows regression tests require registryRead for ordinary child
/// creation and verify that file, network and service-handle denials remain.
fn registry_read_capability()->Result<LocalAllocation>{
    let name=wide_text("registryRead")?;
    let(mut groups,mut capabilities)=(ptr::null_mut(),ptr::null_mut());
    let(mut group_count,mut capability_count)=(0u32,0u32);
    let result=unsafe{DeriveCapabilitySidsFromName(name.as_ptr(),&mut groups,&mut group_count,
        &mut capabilities,&mut capability_count)};
    let error=std::io::Error::last_os_error();
    let _groups=CapabilitySidArray{pointer:groups,count:group_count};
    let capabilities=CapabilitySidArray{pointer:capabilities,count:capability_count};
    if result==0{return Err(error).context("Derive the fixed registryRead capability");}
    if capabilities.count!=1||capabilities.pointer.is_null(){
        bail!("registryRead must resolve to exactly one capability SID");
    }
    let sid=unsafe{*capabilities.pointer};
    if sid.is_null()||unsafe{IsValidSid(sid)}==0{
        bail!("Windows returned an invalid registryRead capability SID");
    }
    // Transfer only the capability SID; free the unused group SIDs and both
    // array allocations with LocalFree, as required by the native API.
    unsafe{*capabilities.pointer=ptr::null_mut()};
    Ok(LocalAllocation(sid))
}
struct ProfileSid(PSID);
impl Drop for ProfileSid{
    fn drop(&mut self){if !self.0.is_null(){unsafe{FreeSid(self.0)};}}
}
struct ComAllocation(*mut c_void);
impl Drop for ComAllocation{
    fn drop(&mut self){if !self.0.is_null(){unsafe{CoTaskMemFree(self.0)};}}
}
unsafe fn os_string_from_pointer(pointer:*const u16)->Result<OsString>{
    if pointer.is_null(){bail!("Windows returned a null string");}
    let mut length=0usize;
    while length<32768&&unsafe{*pointer.add(length)}!=0{length+=1;}
    if length==32768{bail!("Windows returned an oversized string");}
    Ok(OsString::from_wide(unsafe{std::slice::from_raw_parts(pointer,length)}))
}
fn remove_native_profile(name:&str)->Result<()>{
    let native_name=wide_text(name)?;
    let mut last=0i32;
    for attempt in 0..8{
        last=unsafe{DeleteAppContainerProfile(native_name.as_ptr())};
        if last>=0||last as u32==0x80070490{return Ok(());}
        if attempt<7{std::thread::sleep(Duration::from_millis(100));}
    }
    bail!("AppContainer profile cleanup deferred: HRESULT 0x{:08x}",last as u32)
}
fn remove_profile_marker(path:&Path)->Result<()>{
    match fs::remove_file(path){
        Ok(())=>Ok(()),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Ok(()),
        Err(error)=>Err(error).context("Clear command profile recovery marker"),
    }
}

/// Unique identity whose protected recovery marker precedes OS profile creation.
/// SID bytes are owned (rather than a raw pointer), so this object can safely
/// move with an asynchronous Job between executor threads.
pub(super) struct Identity{
    name:String,
    sid:Vec<usize>,
    sid_string:String,
    directory:PathBuf,
    journal:PathBuf,
    record:Arc<Record>,
    active:Arc<AtomicBool>,
    deferred:bool,
    finished:bool,
}
impl Identity{
    pub(super) fn create(state_dir:&Path)->Result<Self>{
        let random=crate::util::random_secret()?;
        let name=format!("EndlessVibe.Isolated.{}",&crate::util::digest(random)[..32]);
        let directory=crate::tools::windows_recovery::journal_dir(state_dir);
        // mark_pending creates the journal atomically with its protected
        // native DACL. Do not pre-create it with ordinary inherited ACLs.
        let journal=crate::tools::windows_recovery::mark_pending(&directory,&name)?;
        let mut may_have_created_profile=false;
        let result=(||->Result<Self>{
            let native_name=wide_text(&name)?;
            let display=wide_text("EndlessVibe isolated command")?;
            let description=wide_text("Per-job offline Less Privileged AppContainer")?;
            let mut sid:PSID=ptr::null_mut();
            may_have_created_profile=true;
            let result=unsafe{CreateAppContainerProfile(
                native_name.as_ptr(),display.as_ptr(),description.as_ptr(),ptr::null(),0,&mut sid,
            )};
            let sid=ProfileSid(sid);
            if result<0{
                // A reported name collision is not a profile owned by this
                // transaction. Do not delete a pre-existing registration.
                if result as u32==0x800700b7{may_have_created_profile=false;}
                bail!("Create isolated command profile failed: HRESULT 0x{:08x}",result as u32);
            }
            if sid.0.is_null()||unsafe{IsValidSid(sid.0)}==0{
                bail!("Windows created a profile without a valid SID");
            }
            let bytes=unsafe{GetLengthSid(sid.0)} as usize;
            if bytes==0||bytes>1024{bail!("AppContainer SID has an unexpected size");}
            let mut owned_sid=vec![0usize;bytes.div_ceil(size_of::<usize>())];
            unsafe{ptr::copy_nonoverlapping(sid.0 as *const u8,owned_sid.as_mut_ptr() as *mut u8,bytes)};
            let mut sid_text:*mut u16=ptr::null_mut();
            if unsafe{ConvertSidToStringSidW(sid.0,&mut sid_text)}==0{
                return Err(std::io::Error::last_os_error()).context("Render command AppContainer SID");
            }
            let sid_memory=LocalAllocation(sid_text as _);
            let sid_string=unsafe{os_string_from_pointer(sid_text)}?.into_string()
                .map_err(|_|anyhow::anyhow!("Windows SID was not valid Unicode"))?;
            let mut profile_path:*mut u16=ptr::null_mut();
            let result=unsafe{GetAppContainerFolderPath(sid_text,&mut profile_path)};
            let profile_memory=ComAllocation(profile_path as _);
            drop(sid_memory);
            if result<0{
                bail!("Resolve command AppContainer folder failed: HRESULT 0x{:08x}",result as u32);
            }
            let directory=PathBuf::from(unsafe{os_string_from_pointer(profile_path)}?);
            drop(profile_memory);
            if !directory.is_absolute(){bail!("AppContainer profile path is not absolute");}
            // A durable prepared record is mandatory before the caller can
            // grant project/toolchain ACLs to this identity.
            let record=Arc::new(Record::prepare(state_dir,&name,&sid_string)?);
            Ok(Self{name:name.clone(),sid:owned_sid,sid_string,directory,journal:journal.clone(),record,
                active:Arc::new(AtomicBool::new(false)),deferred:false,finished:false})
        })();
        match result{
            Ok(identity)=>Ok(identity),
            Err(error)=>{
                // The identity has not escaped and no process or external ACL
                // lease exists. Roll back explicitly instead of relying on a
                // destructor for a Self that was never fully constructed.
                let cleanup=(||->Result<()>{
                    if may_have_created_profile{remove_native_profile(&name)?;}
                    remove_profile_marker(&journal)
                })();
                match cleanup{
                    Ok(())=>Err(error),
                    Err(cleanup)=>Err(crate::error::coded("SANDBOX_CLEANUP_PENDING",false,
                        format!("Windows command identity creation failed ({error:#}); profile cleanup could not be confirmed ({cleanup:#}); recovery markers retained"))),
                }
            },
        }
    }
    pub(super) fn sid(&self)->PSID{self.sid.as_ptr() as PSID}
    pub(super) fn sid_string(&self)->&str{&self.sid_string}
    pub(super) fn directory(&self)->&Path{&self.directory}
    /// True only before a process is created or after the native runner has
    /// confirmed that both its root process and the complete Job have exited.
    /// Callers must still drop RunningCommand before removing leases/profile.
    pub(super) fn job_empty(&self)->bool{
        !self.active.load(Ordering::Acquire)&&self.record.cleanup_ready()
    }
    /// Retain the marker when a caller's ACL lease could not be rolled back.
    pub(super) fn defer_cleanup(&mut self){self.deferred=true;}
    pub(super) fn finish(&mut self)->Result<()>{
        if self.finished{return Ok(());}
        if !self.job_empty(){
            self.deferred=true;
            bail!("AppContainer cleanup requires durable proof that the complete Job Object exited");
        }
        if self.deferred{bail!("AppContainer cleanup deferred; protected recovery marker retained");}
        remove_native_profile(&self.name)?;
        remove_profile_marker(&self.journal)?;
        self.record.finish()?;
        self.finished=true;
        Ok(())
    }
}
impl Drop for Identity{
    fn drop(&mut self){
        if !self.finished&&!self.deferred&&self.job_empty(){
            if let Err(error)=self.finish(){
                tracing::warn!(error=%error,"Command AppContainer profile cleanup deferred");
            }
        }
    }
}

/// All values are prepared by the privileged policy layer, not by a lowbox
/// child. The environment is complete; no service environment is inherited.
#[derive(Clone,Debug)]
pub(super) struct CommandSpec{
    pub(super) program:PathBuf,
    pub(super) args:Vec<OsString>,
    pub(super) cwd:PathBuf,
    pub(super) environment:BTreeMap<OsString,OsString>,
    pub(super) memory_limit_mb:u64,
    pub(super) max_processes:u64,
}

/// Start with OS-derived loader paths and this job's own profile. The policy
/// layer may then add only explicitly approved toolchain/project variables.
pub(super) fn base_environment(identity:&Identity)->Result<BTreeMap<OsString,OsString>>{
    let system=crate::tools::windows_trusted_worker::system32_directory()?;
    let windows=system.parent().context("Windows system directory has no parent")?;
    let drive=match system.components().next(){
        Some(std::path::Component::Prefix(prefix))=>match prefix.kind(){
            std::path::Prefix::Disk(letter)|std::path::Prefix::VerbatimDisk(letter)=>letter,
            _=>bail!("Windows system directory must use a local drive"),
        },
        _=>bail!("Windows system directory has no drive prefix"),
    };
    // Windows redirects LOCALAPPDATA/TEMP/TMP when creating an AppContainer.
    // Supplying the already redirected AC folder as LOCALAPPDATA makes Windows
    // append Packages/<name>/AC again. Derive its input base from the native
    // profile, without inheriting an arbitrary service environment value.
    let package=identity.directory().parent().context("Native AppContainer profile has no package parent")?;
    let packages=package.parent().context("Native AppContainer profile has no Packages parent")?;
    let local=packages.parent().context("Native AppContainer profile has no local application data base")?;
    let named=|path:&Path,name:&str|path.file_name().and_then(OsStr::to_str)
        .is_some_and(|part|part.eq_ignore_ascii_case(name));
    if !named(identity.directory(),"AC")||!named(package,&identity.name)||!named(packages,"Packages"){
        bail!("Unexpected native AppContainer profile layout; refusing to guess its environment base");
    }
    let temporary=identity.directory().join("Temp");
    fs::create_dir_all(&temporary).context("Create private AppContainer temporary directory")?;
    let mut values=BTreeMap::new();
    for (key,value) in [
        ("SystemRoot",windows.as_os_str()),("WINDIR",windows.as_os_str()),
        ("PATH",system.as_os_str()),("HOME",identity.directory().as_os_str()),
        ("USERPROFILE",identity.directory().as_os_str()),
        // This is only the OS redirection input, not an ACL grant to the base.
        ("LOCALAPPDATA",local.as_os_str()),
        ("APPDATA",identity.directory().as_os_str()),
        ("TEMP",temporary.as_os_str()),("TMP",temporary.as_os_str()),
    ]{values.insert(OsString::from(key),value.to_owned());}
    values.insert(OsString::from("SystemDrive"),OsString::from(format!("{}:",(drive as char).to_ascii_uppercase())));
    Ok(values)
}

/// Quote one argument using the Windows CRT backslash/quote contract.
/// Each argument is quoted, including empty strings and trailing backslashes.
fn append_argument(out:&mut Vec<u16>,argument:&OsStr)->Result<()>{
    let units=argument.encode_wide().collect::<Vec<_>>();
    if units.contains(&0){bail!("Windows command argument contains NUL");}
    out.push(b'"' as u16);
    let mut slashes=0usize;
    for unit in units{
        if unit==b'\\' as u16{slashes+=1;continue;}
        if unit==b'"' as u16{
            out.extend(std::iter::repeat(b'\\' as u16).take(slashes*2+1));
        }else{
            out.extend(std::iter::repeat(b'\\' as u16).take(slashes));
        }
        slashes=0;
        out.push(unit);
    }
    out.extend(std::iter::repeat(b'\\' as u16).take(slashes*2));
    out.push(b'"' as u16);
    Ok(())
}
fn command_line(spec:&CommandSpec)->Result<Vec<u16>>{
    let mut line=Vec::new();
    append_argument(&mut line,spec.program.as_os_str())?;
    for argument in &spec.args{
        line.push(b' ' as u16);
        append_argument(&mut line,argument)?;
    }
    if line.len()>=32767{bail!("Windows command line exceeds 32766 UTF-16 units");}
    line.push(0);
    Ok(line)
}
fn environment_block(values:&BTreeMap<OsString,OsString>)->Result<Vec<u16>>{
    let mut entries=Vec::with_capacity(values.len());
    let mut names=std::collections::BTreeSet::new();
    for (key,value) in values{
        let key=key.to_str().context("Windows environment name is not Unicode")?;
        if key.is_empty()||key.contains('=')||key.contains('\0')||!key.is_ascii(){
            bail!("Windows environment names must be nonempty ASCII without '=' or NUL");
        }
        let folded=key.to_ascii_uppercase();
        if !names.insert(folded.clone()){
            bail!("Windows environment contains duplicate case-insensitive names");
        }
        let mut entry=key.encode_utf16().collect::<Vec<_>>();
        entry.push(b'=' as u16);
        let native=value.encode_wide().collect::<Vec<_>>();
        if native.contains(&0){bail!("Windows environment value contains NUL");}
        entry.extend(native);
        entry.push(0);
        entries.push((folded,entry));
    }
    entries.sort_by(|left,right|left.0.cmp(&right.0));
    let mut block=Vec::new();
    for(_,entry)in entries{block.extend(entry);}
    if block.is_empty(){block.push(0);}
    block.push(0);
    if block.len()>32767{bail!("Windows command environment exceeds its size limit");}
    Ok(block)
}

struct NativeJob{handle:OwnedHandle}
impl NativeJob{
    fn new(memory_mb:u64,max_processes:u64)->Result<Self>{
        let bytes=memory_mb.checked_mul(1024*1024)
            .and_then(|value|usize::try_from(value).ok())
            .context("Windows sandbox memory limit exceeds architecture capacity")?;
        let processes=u32::try_from(max_processes)
            .context("Windows sandbox process limit exceeds u32")?;
        if bytes==0||processes==0{bail!("Windows sandbox limits must be positive");}
        let raw=unsafe{CreateJobObjectW(ptr::null(),ptr::null())};
        if raw.is_null(){return Err(std::io::Error::last_os_error()).context("Create isolated command Job Object");}
        let handle=unsafe{OwnedHandle::from_raw_handle(raw as _)};
        let mut info:JOBOBJECT_EXTENDED_LIMIT_INFORMATION=unsafe{zeroed()};
        info.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            |JOB_OBJECT_LIMIT_ACTIVE_PROCESS|JOB_OBJECT_LIMIT_JOB_MEMORY;
        info.BasicLimitInformation.ActiveProcessLimit=processes;
        info.JobMemoryLimit=bytes;
        if unsafe{SetInformationJobObject(handle.as_raw_handle() as _,
            JobObjectExtendedLimitInformation,&info as *const _ as _,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32)}==0{
            return Err(std::io::Error::last_os_error()).context("Set isolated command Job Object limits");
        }
        Ok(Self{handle})
    }
    fn verify_membership(&self,process:HANDLE)->Result<()>{
        let mut contained=0;
        if unsafe{IsProcessInJob(process,self.handle.as_raw_handle() as _,&mut contained)}==0{
            return Err(std::io::Error::last_os_error()).context("Verify AppContainer Job Object membership");
        }
        if contained==0{bail!("AppContainer process was not contained by its Job Object");}
        Ok(())
    }
    fn terminate(&self)->Result<()>{
        if unsafe{TerminateJobObject(self.handle.as_raw_handle() as _,1)}==0{
            return Err(std::io::Error::last_os_error()).context("Terminate contained Windows process tree");
        }
        Ok(())
    }
    fn active_processes(&self)->Result<u32>{
        let mut info:JOBOBJECT_BASIC_ACCOUNTING_INFORMATION=unsafe{zeroed()};
        let mut returned=0;
        if unsafe{QueryInformationJobObject(self.handle.as_raw_handle() as _,
            JobObjectBasicAccountingInformation,&mut info as *mut _ as _,
            size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,&mut returned)}==0{
            return Err(std::io::Error::last_os_error()).context("Confirm complete Windows sandbox process-tree exit");
        }
        if returned as usize!=size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>(){
            bail!("Windows returned incomplete Job Object accounting");
        }
        Ok(info.ActiveProcesses)
    }
}

/// Attribute values are heap-backed for their full Windows-required lifetime.
/// `UpdateProcThreadAttribute` keeps pointers instead of copying every value.
struct Attributes{
    storage:Vec<usize>,
    _caps:Box<SECURITY_CAPABILITIES>,
    _capability_entries:Box<[SID_AND_ATTRIBUTES]>,
    _policy:Box<u32>,
    _handles:Box<[HANDLE;3]>,
    _jobs:Box<[HANDLE;1]>,
}
impl Attributes{
    fn new(identity:&Identity,handles:[HANDLE;3],job:HANDLE,capabilities:&[SID_AND_ATTRIBUTES])->Result<Self>{
        let mut capability_entries=capabilities.to_vec().into_boxed_slice();
        let caps=Box::new(SECURITY_CAPABILITIES{
            AppContainerSid:identity.sid(),
            Capabilities:if capability_entries.is_empty(){ptr::null_mut()}else{capability_entries.as_mut_ptr()},
            CapabilityCount:u32::try_from(capability_entries.len()).context("Too many command capabilities")?,Reserved:0,
        });
        let policy=Box::new(PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT);
        let handles=Box::new(handles);
        let jobs=Box::new([job]);
        let mut bytes=0usize;
        unsafe{InitializeProcThreadAttributeList(ptr::null_mut(),4,0,&mut bytes)};
        if bytes==0{bail!("Windows returned no AppContainer attribute storage");}
        let mut storage=vec![0usize;bytes.div_ceil(size_of::<usize>())];
        if unsafe{InitializeProcThreadAttributeList(storage.as_mut_ptr() as _,4,0,&mut bytes)}==0{
            return Err(std::io::Error::last_os_error()).context("Initialize LPAC command attributes");
        }
        let mut attributes=Self{storage,_caps:caps,_capability_entries:capability_entries,
            _policy:policy,_handles:handles,_jobs:jobs};
        let values=[
            (PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
                &*attributes._caps as *const _ as *mut c_void,size_of::<SECURITY_CAPABILITIES>()),
            (PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
                &*attributes._policy as *const _ as *mut c_void,size_of::<u32>()),
            (PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
                attributes._handles.as_mut_ptr() as *mut c_void,size_of::<[HANDLE;3]>()),
            // Atomic association closes the crash window between creating a
            // suspended root and separately assigning it to a Job Object.
            // This handle is an attribute value, never an inherited handle.
            (PROC_THREAD_ATTRIBUTE_JOB_LIST,
                attributes._jobs.as_mut_ptr() as *mut c_void,size_of::<[HANDLE;1]>()),
        ];
        for (key,value,length) in values{
            if unsafe{UpdateProcThreadAttribute(attributes.pointer(),0,key as usize,
                value,length,ptr::null_mut(),ptr::null())}==0{
                return Err(std::io::Error::last_os_error()).context("Set required LPAC/handle-list attribute");
            }
        }
        Ok(attributes)
    }
    fn pointer(&mut self)->*mut c_void{self.storage.as_mut_ptr() as _}
}
impl Drop for Attributes{
    fn drop(&mut self){unsafe{DeleteProcThreadAttributeList(self.storage.as_mut_ptr() as _)};}
}

fn pipe_pair()->Result<(OwnedHandle,OwnedHandle)>{
    let security=SECURITY_ATTRIBUTES{nLength:size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor:ptr::null_mut(),bInheritHandle:1};
    let(mut read,mut write)=(ptr::null_mut(),ptr::null_mut());
    if unsafe{CreatePipe(&mut read,&mut write,&security,0)}==0{
        return Err(std::io::Error::last_os_error()).context("Create isolated command standard pipe");
    }
    Ok((unsafe{OwnedHandle::from_raw_handle(read as _)},unsafe{OwnedHandle::from_raw_handle(write as _)}))
}
fn noninheritable(handle:&OwnedHandle)->Result<()>{
    if unsafe{SetHandleInformation(handle.as_raw_handle() as _,HANDLE_FLAG_INHERIT,0)}==0{
        return Err(std::io::Error::last_os_error()).context("Remove broker pipe-handle inheritance");
    }
    Ok(())
}
struct PipeReader{file:fs::File,closed:bool}
impl PipeReader{
    fn new(handle:OwnedHandle)->Self{Self{file:fs::File::from(handle),closed:false}}
    /// Peek first and read no more than bytes already queued: no blocking IO
    /// threads may survive cancellation or keep profile cleanup alive.
    fn drain(&mut self,label:&'static str,on_output:&mut impl FnMut(&'static str,&[u8])->Result<()>)->Result<()>{
        if self.closed{return Ok(());}
        let mut page=[0u8;8192];
        for _ in 0..8{
            let mut available=0u32;
            if unsafe{PeekNamedPipe(self.file.as_raw_handle() as _,ptr::null_mut(),0,
                ptr::null_mut(),&mut available,ptr::null_mut())}==0{
                let error=std::io::Error::last_os_error();
                if matches!(error.raw_os_error(),Some(code) if
                    code==ERROR_BROKEN_PIPE as i32||code==ERROR_NO_DATA as i32||code==ERROR_PIPE_NOT_CONNECTED as i32){
                    self.closed=true;return Ok(());
                }
                return Err(error).context("Read isolated command pipe availability");
            }
            if available==0{return Ok(());}
            let limit=page.len().min(available as usize);
            let count=self.file.read(&mut page[..limit]).context("Read isolated command standard output")?;
            if count==0{self.closed=true;return Ok(());}
            on_output(label,&page[..count])?;
        }
        Ok(())
    }
}

fn token_blob(token:HANDLE,class:windows_sys::Win32::Security::TOKEN_INFORMATION_CLASS)->Result<Vec<usize>>{
    let mut bytes=0u32;
    let status=unsafe{GetTokenInformation(token,class,ptr::null_mut(),0,&mut bytes)};
    if status!=0||std::io::Error::last_os_error().raw_os_error()!=Some(ERROR_INSUFFICIENT_BUFFER as i32)
        ||bytes==0||bytes as usize>MAX_TOKEN_BYTES{
        bail!("Windows token query did not return a bounded information size");
    }
    let mut buffer=vec![0usize;(bytes as usize).div_ceil(size_of::<usize>())];
    let capacity=buffer.len()*size_of::<usize>();
    let mut returned=0u32;
    if unsafe{GetTokenInformation(token,class,buffer.as_mut_ptr() as _,capacity as u32,&mut returned)}==0{
        return Err(std::io::Error::last_os_error()).context("Read isolated Windows process token");
    }
    if returned==0||returned as usize>capacity{bail!("Windows returned malformed token information");}
    Ok(buffer)
}
fn token_dword(token:HANDLE,class:windows_sys::Win32::Security::TOKEN_INFORMATION_CLASS)->Result<u32>{
    let(mut value,mut returned)=(0u32,0u32);
    if unsafe{GetTokenInformation(token,class,&mut value as *mut _ as _,size_of::<u32>() as u32,&mut returned)}==0{
        return Err(std::io::Error::last_os_error()).context("Read isolated token property");
    }
    if returned as usize!=size_of::<u32>(){bail!("Windows returned an incomplete token property");}
    Ok(value)
}
fn access_check(token:HANDLE,sddl:&str)->Result<bool>{
    let sddl=wide_text(sddl)?;
    let mut security=ptr::null_mut();
    if unsafe{ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(),1,&mut security,ptr::null_mut())}==0{
        return Err(std::io::Error::last_os_error()).context("Create LPAC access-check descriptor");
    }
    let _security=LocalAllocation(security);
    let mut duplicate=ptr::null_mut();
    if unsafe{DuplicateToken(token,SecurityImpersonation,&mut duplicate)}==0{
        return Err(std::io::Error::last_os_error()).context("Duplicate LPAC token for access check");
    }
    let duplicate=unsafe{OwnedHandle::from_raw_handle(duplicate as _)};
    let mut mapping=GENERIC_MAPPING{GenericRead:FILE_GENERIC_READ,GenericWrite:FILE_GENERIC_WRITE,
        GenericExecute:FILE_GENERIC_EXECUTE,GenericAll:FILE_ALL_ACCESS};
    let mut privilege_buffer=vec![0usize;1024usize.div_ceil(size_of::<usize>())];
    let mut privilege_bytes=(privilege_buffer.len()*size_of::<usize>()) as u32;
    let(mut granted,mut allowed)=(0u32,0i32);
    let ok=unsafe{AccessCheck(security,duplicate.as_raw_handle() as _,FILE_GENERIC_READ,
        &mut mapping,privilege_buffer.as_mut_ptr() as *mut PRIVILEGE_SET,
        &mut privilege_bytes,&mut granted,&mut allowed)};
    if ok==0{return Err(std::io::Error::last_os_error()).context("Check LPAC resource-access semantics");}
    if allowed!=0&&granted&FILE_GENERIC_READ!=FILE_GENERIC_READ{
        bail!("Windows returned inconsistent access-check permissions");
    }
    Ok(allowed!=0)
}
fn verify_capabilities(token:HANDLE,expected:&[SID_AND_ATTRIBUTES])->Result<()>{
    let caps=token_blob(token,TokenCapabilities)?;
    let count=unsafe{*(caps.as_ptr() as *const u32)} as usize;
    if count!=expected.len(){bail!("Windows command received an unexpected capability count");}
    let offset=std::mem::offset_of!(TOKEN_GROUPS,Groups);
    let needed=offset.checked_add(count.checked_mul(size_of::<SID_AND_ATTRIBUTES>())
        .context("Invalid capability array length")?).context("Invalid capability array length")?;
    if needed>caps.len()*size_of::<usize>(){bail!("Windows command capability array is incomplete");}
    let entries=unsafe{std::slice::from_raw_parts(
        (caps.as_ptr() as *const u8).add(offset) as *const SID_AND_ATTRIBUTES,count)};
    for (index,entry) in entries.iter().enumerate(){
        if entry.Sid.is_null()||unsafe{IsValidSid(entry.Sid)}==0{
            bail!("Windows command capability SID is invalid");
        }
        if !expected.iter().any(|allowed|allowed.Attributes==entry.Attributes&&
            unsafe{EqualSid(allowed.Sid,entry.Sid)}!=0){
            bail!("Windows command received a capability outside the exact allowlist");
        }
        if entries[..index].iter().any(|previous|unsafe{EqualSid(previous.Sid,entry.Sid)}!=0){
            bail!("Windows command capability array contains duplicates");
        }
    }
    Ok(())
}
fn verify_token(process:HANDLE,identity:&Identity,capabilities:&[SID_AND_ATTRIBUTES])->Result<()>{
    let mut raw=ptr::null_mut();
    if unsafe{OpenProcessToken(process,TOKEN_QUERY|TOKEN_DUPLICATE,&mut raw)}==0{
        return Err(std::io::Error::last_os_error()).context("Open suspended AppContainer token");
    }
    let token=unsafe{OwnedHandle::from_raw_handle(raw as _)};
    let token=token.as_raw_handle() as HANDLE;
    if token_dword(token,TokenIsAppContainer)?!=1{bail!("Windows command lacks an AppContainer token");}
    let app=token_blob(token,TokenAppContainerSid)?;
    if app.len()*size_of::<usize>()<size_of::<TOKEN_APPCONTAINER_INFORMATION>(){
        bail!("Windows command AppContainer identity is incomplete");
    }
    let app=unsafe{&*(app.as_ptr() as *const TOKEN_APPCONTAINER_INFORMATION)};
    if app.TokenAppContainer.is_null()||unsafe{EqualSid(app.TokenAppContainer,identity.sid())}==0{
        bail!("Windows command has the wrong AppContainer SID");
    }
    verify_capabilities(token,capabilities)?;
    let integrity=token_blob(token,TokenIntegrityLevel)?;
    if integrity.len()*size_of::<usize>()<size_of::<TOKEN_MANDATORY_LABEL>(){
        bail!("Windows command integrity label is incomplete");
    }
    let label=unsafe{&*(integrity.as_ptr() as *const TOKEN_MANDATORY_LABEL)};
    if label.Label.Sid.is_null()||unsafe{IsValidSid(label.Label.Sid)}==0{
        bail!("Windows command has no valid integrity label");
    }
    let count=unsafe{*GetSidSubAuthorityCount(label.Label.Sid)};
    if count==0||unsafe{*GetSidSubAuthority(label.Label.Sid,(count-1) as u32)}>0x1000{
        bail!("Windows command integrity exceeds Low");
    }
    // Win32 does not define a usable TokenIsLessPrivilegedAppContainer query
    // on every supported Windows build. Check the defining access behavior:
    // this exact package SID works, but ALL APPLICATION PACKAGES alone does not.
    let own=format!("O:SYG:SYD:(A;;FR;;;WD)(A;;FR;;;{})",identity.sid_string());
    if !access_check(token,&own)?{
        bail!("AppContainer token failed its positive package-SID access check");
    }
    if access_check(token,"O:SYG:SYD:(A;;FR;;;WD)(A;;FR;;;AC)")?{
        bail!("Windows command is not sufficiently restricted: ALL APPLICATION PACKAGES access remains");
    }
    Ok(())
}

#[derive(Clone,Debug)]
pub(super) struct Outcome{
    pub(super) exit_code:u32,
    pub(super) cancelled:bool,
    pub(super) timed_out:bool,
    pub(super) token_verified:bool,
    pub(super) job_empty:bool,
}
pub(super) struct RunningCommand{
    process:OwnedHandle,
    thread:OwnedHandle,
    job:NativeJob,
    stdout:PipeReader,
    stderr:PipeReader,
    active:Arc<AtomicBool>,
    record:Arc<Record>,
    #[cfg(test)]
    pid:u32,
    resumed:bool,
    empty:bool,
}
impl RunningCommand{
    #[cfg(test)]
    pub(super) fn id(&self)->u32{self.pid}
    fn exited(&self)->Result<bool>{
        match unsafe{WaitForSingleObject(self.process.as_raw_handle() as _,0)}{
            WAIT_OBJECT_0=>Ok(true),WAIT_TIMEOUT=>Ok(false),
            _=>Err(std::io::Error::last_os_error()).context("Poll contained Windows command"),
        }
    }
    async fn stop_tree(&mut self)->Result<()>{
        self.job.terminate()?;
        let deadline=Instant::now()+CLEANUP_TIMEOUT;
        loop{
            if self.exited()?&&self.job.active_processes()?==0{
                self.record.mark_empty()?;
                self.empty=true;
                self.active.store(false,Ordering::Release);
                return Ok(());
            }
            if Instant::now()>=deadline{bail!("Windows sandbox process tree did not exit; cleanup deferred");}
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
    /// Emit raw stdout/stderr chunks. A zero-length stdout callback every
    /// 250ms is a flush tick, not output: quiet commands must still publish
    /// their last pending line using the existing Job output contract.
    pub(super) async fn run(
        &mut self,cancel:&CancellationToken,timeout:Duration,
        mut on_output:impl FnMut(&'static str,&[u8])->Result<()>,
    )->Result<Outcome>{
        if self.resumed{bail!("An AppContainer command cannot be executed twice");}
        if timeout.is_zero(){bail!("Windows sandbox timeout must be positive");}
        let mut cancelled=cancel.is_cancelled();
        let mut timed_out=false;
        let mut early_error:Option<anyhow::Error>=None;
        if !cancelled{
            if unsafe{ResumeThread(self.thread.as_raw_handle() as _)}!=1{
                early_error=Some(anyhow::anyhow!("Windows sandbox primary thread did not have its expected suspension count"));
            }
        }
        self.resumed=true;
        let deadline=Instant::now()+timeout;
        let mut last_tick=Instant::now();
        while early_error.is_none()&&!cancelled&&!timed_out{
            let observation=(||->Result<bool>{
                self.stdout.drain("stdout",&mut on_output)?;
                self.stderr.drain("stderr",&mut on_output)?;
                if last_tick.elapsed()>=Duration::from_millis(250){
                    on_output("stdout",&[])?;
                    last_tick=Instant::now();
                }
                self.exited()
            })();
            match observation{
                Ok(true)=>break,Ok(false)=>{},Err(error)=>{early_error=Some(error);break;}
            }
            tokio::select!{
                _=cancel.cancelled()=>cancelled=true,
                _=tokio::time::sleep(POLL_INTERVAL)=>timed_out=Instant::now()>=deadline,
            }
        }
        let mut exit_code=1u32;
        // Capture the primary exit code before terminating still-live descendants.
        match self.exited(){
            Ok(true)=>{
                if unsafe{GetExitCodeProcess(self.process.as_raw_handle() as _,&mut exit_code)}==0{
                    early_error=Some(std::io::Error::last_os_error().into());
                }
            },
            Ok(false)=>{},
            Err(error)=>early_error=Some(error),
        }
        self.stop_tree().await?;
        if !cancelled&&!timed_out&&early_error.is_none(){
            let drain_deadline=Instant::now()+Duration::from_secs(2);
            while !(self.stdout.closed&&self.stderr.closed){
                self.stdout.drain("stdout",&mut on_output)?;
                self.stderr.drain("stderr",&mut on_output)?;
                if self.stdout.closed&&self.stderr.closed{break;}
                if Instant::now()>=drain_deadline{
                    bail!("Windows sandbox pipes remained open after the complete Job Object exited");
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }else if early_error.is_none(){
            self.stdout.drain("stdout",&mut on_output)?;
            self.stderr.drain("stderr",&mut on_output)?;
        }
        if let Some(error)=early_error{return Err(error);}
        Ok(Outcome{exit_code,cancelled,timed_out,token_verified:true,job_empty:self.empty})
    }
}
impl Drop for RunningCommand{
    fn drop(&mut self){
        if self.empty{return;}
        // Covers task abortion/panic before or during run(). No async reader
        // tasks exist, and the non-inheritable Job handle dies with this owner.
        let _=self.job.terminate();
        let _=unsafe{TerminateProcess(self.process.as_raw_handle() as _,1)};
        let deadline=Instant::now()+CLEANUP_TIMEOUT;
        while Instant::now()<deadline{
            if self.exited().ok()==Some(true)&&self.job.active_processes().ok()==Some(0){
                match self.record.mark_empty(){
                    Ok(())=>{
                        self.empty=true;
                        self.active.store(false,Ordering::Release);
                    },
                    Err(error)=>tracing::warn!(error=%error,
                        "Windows process tree exited but durable cleanup proof failed; recovery record retained"),
                }
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

/// Create but do not resume the process. Only three fresh pipe handles cross
/// the privilege boundary; sockets, tokens, config files and Job handles do not.
pub(super) fn spawn(identity:&Identity,spec:&CommandSpec)->Result<RunningCommand>{
    if identity.finished||identity.deferred||!identity.record.cleanup_ready(){
        bail!("AppContainer identity is unavailable or has unconfirmed execution state");
    }
    if !spec.program.is_absolute()||!spec.cwd.is_absolute(){
        bail!("Windows sandbox program and working directory must be absolute");
    }
    let executable=wide(spec.program.as_os_str())?;
    let cwd=wide(spec.cwd.as_os_str())?;
    let mut command=command_line(spec)?;
    let mut environment=environment_block(&spec.environment)?;
    let capability=registry_read_capability()?;
    let capabilities=[SID_AND_ATTRIBUTES{Sid:capability.0,Attributes:CAPABILITY_ENABLED}];
    let job=NativeJob::new(spec.memory_limit_mb,spec.max_processes)?;
    let(stdin_read,stdin_write)=pipe_pair()?;
    let(stdout_read,stdout_write)=pipe_pair()?;
    let(stderr_read,stderr_write)=pipe_pair()?;
    noninheritable(&stdin_write)?;
    noninheritable(&stdout_read)?;
    noninheritable(&stderr_read)?;
    // An immediately closed input pipe is a deterministic EOF; do not attach
    // the service console or open caller-controlled device paths such as NUL.
    drop(stdin_write);
    let handles=[stdin_read.as_raw_handle() as HANDLE,stdout_write.as_raw_handle() as HANDLE,
        stderr_write.as_raw_handle() as HANDLE];
    let mut attributes=Attributes::new(identity,handles,job.handle.as_raw_handle() as _,&capabilities)?;
    let mut startup:STARTUPINFOEXW=unsafe{zeroed()};
    startup.StartupInfo.cb=size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags=STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput=handles[0];
    startup.StartupInfo.hStdOutput=handles[1];
    startup.StartupInfo.hStdError=handles[2];
    startup.lpAttributeList=attributes.pointer() as _;
    let mut info:PROCESS_INFORMATION=unsafe{zeroed()};
    if identity.active.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){
        bail!("AppContainer identity is already running a command");
    }
    if let Err(error)=identity.record.mark_running(){
        identity.active.store(false,Ordering::Release);
        return Err(error).context("Record Windows sandbox execution before process creation");
    }
    let result=unsafe{CreateProcessW(executable.as_ptr(),command.as_mut_ptr(),ptr::null(),ptr::null(),1,
        CREATE_SUSPENDED|CREATE_UNICODE_ENVIRONMENT|EXTENDED_STARTUPINFO_PRESENT|CREATE_NO_WINDOW,
        environment.as_mut_ptr() as _,cwd.as_ptr(),&startup.StartupInfo,&mut info)};
    if result==0{
        let error=std::io::Error::last_os_error();
        // No process exists on CreateProcess failure. Persist that fact before
        // allowing the caller to revoke leases or remove the profile.
        if let Err(proof)=identity.record.mark_empty(){
            return Err(crate::error::coded("SANDBOX_CLEANUP_PENDING",false,
                format!("Create offline LPAC command failed ({error}); recording the absence of a process also failed ({proof:#})")));
        }
        identity.active.store(false,Ordering::Release);
        return Err(error).context("Create offline LPAC command; no host fallback was attempted");
    }
    let running=RunningCommand{
        process:unsafe{OwnedHandle::from_raw_handle(info.hProcess as _)},
        thread:unsafe{OwnedHandle::from_raw_handle(info.hThread as _)},
        job,stdout:PipeReader::new(stdout_read),stderr:PipeReader::new(stderr_read),
        active:identity.active.clone(),record:identity.record.clone(),
        #[cfg(test)]
        pid:info.dwProcessId,
        resumed:false,empty:false,
    };
    // On either failure RunningCommand::drop kills the still-suspended child.
    running.job.verify_membership(running.process.as_raw_handle() as _)?;
    verify_token(running.process.as_raw_handle() as _,identity,&capabilities)?;
    drop(attributes);
    drop(stdin_read);
    drop(stdout_write);
    drop(stderr_write);
    Ok(running)
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::{io::Write,net::{TcpListener,TcpStream},os::windows::fs::OpenOptionsExt,
        process::{Command,Stdio}};
    use windows_sys::Win32::{
        Foundation::{CompareObjectHandles,DuplicateHandle,DUPLICATE_SAME_ACCESS,
            ERROR_INVALID_HANDLE,ERROR_PROC_NOT_FOUND,FARPROC,GetLastError,HMODULE},
        Networking::WinSock::{
            AF_INET,INVALID_SOCKET,IPPROTO_TCP,SOCKADDR_IN,SOCK_STREAM,SOCKET_ERROR,
            WSAEACCES,WSASYSCALLFAILURE,WSADATA,WSACleanup,WSAGetLastError,WSAStartup,
            closesocket,connect,socket,
        },
        Security::{AddAce,AddAccessAllowedAceEx,GetAce,InitializeAcl,ACL,
            DACL_SECURITY_INFORMATION,GetSecurityDescriptorDacl,
            PROTECTED_DACL_SECURITY_INFORMATION,
            Authorization::{GetSecurityInfo,SE_FILE_OBJECT,SetNamedSecurityInfoW,SetSecurityInfo}},
        Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION,FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT,FILE_FLAG_BACKUP_SEMANTICS,FILE_FLAG_OPEN_REPARSE_POINT,
            GetFileInformationByHandle,GetFinalPathNameByHandleW,GetTempPathW},
        System::{
            Console::{GetStdHandle,STD_INPUT_HANDLE},
            Threading::{CreateEventW,GetCurrentProcess,OpenProcess,
                PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_VM_READ},
        },
    };
    // GetTempPath2W is absent on older supported Windows installations. Resolve
    // it at runtime so the native regression can still test GetTempPathW there.
    #[link(name="kernel32")]
    unsafe extern "system"{
        fn GetModuleHandleW(name:*const u16)->HMODULE;
        fn GetProcAddress(module:HMODULE,name:*const u8)->FARPROC;
        fn CompareStringOrdinal(a:*const u16,a_len:i32,b:*const u16,b_len:i32,ignore_case:i32)->i32;
    }
    const ENTRY:&str="tools::windows_command::tests::isolated_command_test_entry";
    fn assert_service_handle_not_inherited(running:&RunningCommand,canary:&OwnedHandle){
        let mut positive=ptr::null_mut();
        assert_ne!(unsafe{DuplicateHandle(GetCurrentProcess(),canary.as_raw_handle() as _,
            GetCurrentProcess(),&mut positive,0,0,DUPLICATE_SAME_ACCESS)},0);
        let positive=unsafe{OwnedHandle::from_raw_handle(positive as _)};
        assert_ne!(unsafe{CompareObjectHandles(canary.as_raw_handle() as _,positive.as_raw_handle() as _)},0,
            "Event-object identity positive control failed");
        drop(positive);
        let mut duplicated=ptr::null_mut();
        // Inherited handles preserve the original numeric value. Duplicate
        // from the suspended child in the broker, then compare kernel-object
        // identity: a coincidentally equal number is not evidence of a leak.
        let result=unsafe{DuplicateHandle(running.process.as_raw_handle() as _,canary.as_raw_handle() as _,
            GetCurrentProcess(),&mut duplicated,0,0,DUPLICATE_SAME_ACCESS)};
        if result==0{
            assert_eq!(unsafe{GetLastError()},ERROR_INVALID_HANDLE,
                "Broker could not inspect the suspended child's handle table");
        }else{
            let duplicated=unsafe{OwnedHandle::from_raw_handle(duplicated as _)};
            assert_eq!(unsafe{CompareObjectHandles(canary.as_raw_handle() as _,duplicated.as_raw_handle() as _)},0,
                "Service-owned event object was inherited by the LPAC process");
        }
        println!("LPAC_SERVICE_HANDLE_TABLE_VERIFIED");
    }

    fn fixture(mode:&str)->(tempfile::TempDir,Identity,CommandSpec){
        let state=tempfile::tempdir().unwrap();
        crate::util::private_dir(state.path()).unwrap();
        let identity=Identity::create(state.path()).unwrap();
        let program=identity.directory().join("command-test.exe");
        let mut source=fs::File::open(std::env::current_exe().unwrap()).unwrap();
        let mut destination=fs::OpenOptions::new().write(true).create_new(true).open(&program).unwrap();
        std::io::copy(&mut source,&mut destination).unwrap();
        destination.sync_all().unwrap();
        drop(destination);
        let mut environment=base_environment(&identity).unwrap();
        environment.insert("ENDLESSVIBE_COMMAND_TEST_MODE".into(),mode.into());
        environment.insert("ENDLESSVIBE_COMMAND_TEST_ROOT".into(),identity.directory().as_os_str().to_owned());
        environment.insert("ENDLESSVIBE_COMMAND_TEST_SID".into(),identity.sid_string().into());
        let spec=CommandSpec{program,args:vec!["--exact".into(),ENTRY.into(),"--nocapture".into(),"--test-threads=1".into()],
            cwd:identity.directory().to_owned(),environment,memory_limit_mb:512,max_processes:8};
        (state,identity,spec)
    }
    fn marker_pid(output:&[u8],marker:&str)->Option<u32>{
        let text=String::from_utf8_lossy(output);
        let suffix=text.split_once(marker)?.1;
        let end=suffix.find(|character:char|!character.is_ascii_digit())?;
        let digits=&suffix[..end];
        (!digits.is_empty()).then(||digits.parse().ok()).flatten()
    }
    fn child_handle(pid:u32)->OwnedHandle{
        const SYNCHRONIZE:u32=0x00100000;
        let raw=unsafe{OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|SYNCHRONIZE,0,pid)};
        assert!(!raw.is_null(),"Cannot observe test descendant: {}",std::io::Error::last_os_error());
        unsafe{OwnedHandle::from_raw_handle(raw as _)}
    }
    fn assert_exited(handle:&OwnedHandle){
        assert_eq!(unsafe{WaitForSingleObject(handle.as_raw_handle() as _,0)},WAIT_OBJECT_0,
            "A real descendant survived sandbox completion");
    }
    fn write_aap_only_file(path:&Path){
        fs::write(path,b"AAP-only test canary").unwrap();
        let descriptor=wide_text("D:P(A;;FA;;;SY)(A;;FR;;;WD)(A;;FR;;;AC)").unwrap();
        let mut security=ptr::null_mut();
        assert_ne!(unsafe{ConvertStringSecurityDescriptorToSecurityDescriptorW(
            descriptor.as_ptr(),1,&mut security,ptr::null_mut())},0);
        let _allocation=LocalAllocation(security);
        let(mut present,mut defaulted)=(0i32,0i32);
        let mut dacl=ptr::null_mut();
        assert_ne!(unsafe{GetSecurityDescriptorDacl(security,&mut present,&mut dacl,&mut defaulted)},0);
        assert_ne!(present,0);
        let path=wide(path.as_os_str()).unwrap();
        assert_eq!(unsafe{SetNamedSecurityInfoW(path.as_ptr(),SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION|PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),ptr::null_mut(),dacl,ptr::null_mut())},0);
    }
    fn spawn_descendant(mode:&str)->std::process::Child{
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact",ENTRY,"--nocapture","--test-threads=1"])
            .env("ENDLESSVIBE_COMMAND_TEST_MODE",mode)
            .stdin(Stdio::inherit()).stdout(Stdio::inherit()).stderr(Stdio::inherit())
            .spawn().expect("Ordinary LPAC child process creation failed")
    }
    fn current_token()->OwnedHandle{
        let mut raw=ptr::null_mut();
        assert_ne!(unsafe{OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY|TOKEN_DUPLICATE,&mut raw)},0);
        unsafe{OwnedHandle::from_raw_handle(raw as _)}
    }
    fn assert_current_lpac(){
        let token=current_token();
        assert_eq!(token_dword(token.as_raw_handle() as _,TokenIsAppContainer).unwrap(),1);
        let capability=registry_read_capability().unwrap();
        verify_capabilities(token.as_raw_handle() as _,
            &[SID_AND_ATTRIBUTES{Sid:capability.0,Attributes:CAPABILITY_ENABLED}]).unwrap();
        let sid=std::env::var("ENDLESSVIBE_COMMAND_TEST_SID").unwrap();
        assert!(access_check(token.as_raw_handle() as _,
            &format!("O:SYG:SYD:(A;;FR;;;WD)(A;;FR;;;{sid})")).unwrap());
        assert!(!access_check(token.as_raw_handle() as _,
            "O:SYG:SYD:(A;;FR;;;WD)(A;;FR;;;AC)").unwrap());
    }
    fn assert_io_denied(label:&str,path:&Path,operation:impl FnOnce()->std::io::Result<()>){
        match operation(){
            Ok(())=>panic!("LPAC unexpectedly allowed {label} at {}",path.display()),
            Err(error)=>assert_eq!(error.kind(),std::io::ErrorKind::PermissionDenied,
                "LPAC {label} at {} failed for a different reason: {error}",path.display()),
        }
    }
    fn assert_native_access_denied(label:&str,path:&Path,operation:impl FnOnce()->std::io::Result<()>){
        let error=operation().expect_err(&format!("LPAC unexpectedly allowed {label} at {}",path.display()));
        // A held broker pin can cause ERROR_SHARING_VIOLATION. It must not
        // substitute for the actual per-SID access denial being tested here.
        assert_eq!(error.raw_os_error(),Some(5),
            "LPAC {label} at {} failed for a reason other than access policy: {error}",path.display());
    }
    fn assert_native_temporary_directory(profile:&Path){
        type TempPathFn=unsafe extern "system" fn(u32,*mut u16)->u32;
        fn comparable(path:&Path)->Vec<u16>{
            let path=path.to_str().expect("Native temporary path is not Unicode");
            path.strip_prefix(r"\\?\").unwrap_or(path).replace('/',"\\")
                .trim_end_matches('\\').encode_utf16().collect()
        }
        fn open_directory(path:&Path)->(fs::File,(u32,u64)){
            // Only ask for file attributes. Canonicalizing to a DOS drive name
            // can require unrelated volume namespace queries inside a lowbox.
            let file=fs::OpenOptions::new().access_mode(0x80)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT).open(path)
                .unwrap_or_else(|error|panic!("Open native Temp directory attributes {}: {error}",path.display()));
            let mut info:BY_HANDLE_FILE_INFORMATION=unsafe{zeroed()};
            assert_ne!(unsafe{GetFileInformationByHandle(file.as_raw_handle() as _,&mut info)},0,
                "Read native Temp directory identity {}: {}",path.display(),std::io::Error::last_os_error());
            assert_ne!(info.dwFileAttributes&FILE_ATTRIBUTE_DIRECTORY,0);
            assert_eq!(info.dwFileAttributes&FILE_ATTRIBUTE_REPARSE_POINT,0,
                "Native temporary directory must not be a reparse point");
            let identity=(info.dwVolumeSerialNumber,((info.nFileIndexHigh as u64)<<32)|info.nFileIndexLow as u64);
            assert!(identity.0!=0&&identity.1!=0,"Native Temp directory has no stable file identity");
            (file,identity)
        }
        let expected=profile.join("Temp");
        let check=|name:&str,api:TempPathFn|{
            let mut buffer=vec![0u16;32768];
            let length=unsafe{api(buffer.len() as u32,buffer.as_mut_ptr())} as usize;
            assert!(length>0&&length<buffer.len(),"{name} failed: {}",std::io::Error::last_os_error());
            let path=PathBuf::from(OsString::from_wide(&buffer[..length]));
            println!("LPAC_NATIVE_TEMP_PATH:{name}:{}",path.display());
            std::io::stdout().flush().unwrap();
            let actual_text=comparable(&path);let expected_text=comparable(&expected);
            assert!(actual_text.len()==expected_text.len()&&unsafe{CompareStringOrdinal(
                actual_text.as_ptr(),actual_text.len() as i32,expected_text.as_ptr(),expected_text.len() as i32,1)}==2,
                "{name} returned {}, expected {}; LOCALAPPDATA={:?} TMP={:?} TEMP={:?}",
                path.display(),expected.display(),std::env::var_os("LOCALAPPDATA"),
                std::env::var_os("TMP"),std::env::var_os("TEMP"));
            let (_expected_handle,expected_identity)=open_directory(&expected);
            let (actual_handle,actual_identity)=open_directory(&path);
            assert_eq!(actual_identity,expected_identity,
                "{name} must identify exactly this job's existing native profile/Temp");
            // Observe the separate DOS/NT name-query behavior without making
            // either namespace query an extra permission requirement for I/O.
            for (kind,flags) in [("DOS",0),("NT",2)]{
                let length=unsafe{GetFinalPathNameByHandleW(actual_handle.as_raw_handle() as _,
                    buffer.as_mut_ptr(),buffer.len() as u32,flags)} as usize;
                if length==0{
                    let error=unsafe{GetLastError()};
                    println!("LPAC_NATIVE_TEMP_PATH_QUERY:{name}:{kind}:ERROR:{error}");
                }else{
                    assert!(length<buffer.len(),"Native temporary path query exceeded its buffer");
                    println!("LPAC_NATIVE_TEMP_PATH_QUERY:{name}:{kind}:{}",OsString::from_wide(&buffer[..length]).to_string_lossy());
                }
            }
            // Do not create a missing directory: a duplicated or redirected
            // path must fail before it can become a compatibility workaround.
            let canary=path.join(format!("native-{name}-{}.tmp",std::process::id()));
            let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&canary).unwrap();
            file.write_all(b"native private temporary file").unwrap();
            file.flush().unwrap();drop(file);
            assert_eq!(fs::read(&canary).unwrap(),b"native private temporary file");
            fs::remove_file(&canary).unwrap();
            println!("LPAC_NATIVE_TEMP_VERIFIED:{name}:{}",path.display());
            std::io::stdout().flush().unwrap();
        };
        check("GetTempPathW",GetTempPathW);
        let module=unsafe{GetModuleHandleW(wide_text("kernel32.dll").unwrap().as_ptr())};
        assert!(!module.is_null(),"The loaded native runtime module was not found");
        match unsafe{GetProcAddress(module,b"GetTempPath2W\0".as_ptr())}{
            Some(function)=>{
                let api:TempPathFn=unsafe{std::mem::transmute(function)};
                check("GetTempPath2W",api);
            },
            None=>{
                assert_eq!(unsafe{GetLastError()},ERROR_PROC_NOT_FOUND);
                println!("LPAC_GET_TEMP_PATH_2_UNAVAILABLE");
                std::io::stdout().flush().unwrap();
            },
        }
    }
    fn attempt_private_parent_acl_propagation(profile:&Path){
        // This is the current job's own native package directory, never the
        // global Packages directory or another application's profile.
        let parent=profile.parent().expect("Native private profile has no package parent");
        let parent=match fs::OpenOptions::new().access_mode(0x0006_0000)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT).open(parent){
            Ok(parent)=>parent,
            Err(error)if error.raw_os_error()==Some(5)=>{
                println!("LPAC_PRIVATE_PARENT_WRITE_DAC_DENIED");
                std::io::stdout().flush().unwrap();
                return;
            },
            Err(error)=>panic!("Private package parent WRITE_DAC probe failed unexpectedly: {error}"),
        };
        let mut security=ptr::null_mut();
        let mut dacl:*mut ACL=ptr::null_mut();
        assert_eq!(unsafe{GetSecurityInfo(parent.as_raw_handle() as _,SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,ptr::null_mut(),ptr::null_mut(),&mut dacl,
            ptr::null_mut(),&mut security)},0);
        let _security=LocalAllocation(security);
        assert!(!dacl.is_null(),"Native package parent has a NULL DACL");
        let token=current_token();
        let app=token_blob(token.as_raw_handle() as _,TokenAppContainerSid).unwrap();
        let app=unsafe{&*(app.as_ptr() as *const TOKEN_APPCONTAINER_INFORMATION)};
        assert!(!app.TokenAppContainer.is_null()&&unsafe{IsValidSid(app.TokenAppContainer)}!=0);
        let bytes=unsafe{(*dacl).AclSize} as usize+8+unsafe{GetLengthSid(app.TokenAppContainer)} as usize;
        assert!(bytes<=u16::MAX as usize,"Native package parent DACL exceeds test bounds");
        let mut storage=vec![0u32;bytes.div_ceil(4)];
        let merged=storage.as_mut_ptr() as *mut ACL;
        assert_ne!(unsafe{InitializeAcl(merged,bytes as u32,4)},0);
        // Add a new inheritable FULL allow, so this is a concrete attempt to
        // relax descendants, not a no-op rewrite of an identical descriptor.
        assert_ne!(unsafe{AddAccessAllowedAceEx(merged,4,0x03,FILE_ALL_ACCESS,app.TokenAppContainer)},0);
        for index in 0..unsafe{(*dacl).AceCount} as u32{
            let mut ace=ptr::null_mut();
            assert_ne!(unsafe{GetAce(dacl,index,&mut ace)},0);
            let length=unsafe{ptr::read_unaligned((ace as *const u8).add(2) as *const u16)} as u32;
            assert_ne!(unsafe{AddAce(merged,4,u32::MAX,ace,length)},0);
        }
        // The Win32 API deliberately propagates inheritable ACEs. A protected
        // AC root must keep this parent-owned change out of the sealed tools.
        assert_eq!(unsafe{SetSecurityInfo(parent.as_raw_handle() as _,SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,ptr::null_mut(),ptr::null_mut(),merged,ptr::null_mut())},0,
            "A writable private package parent could not perform the inheritance attack");
        println!("LPAC_PRIVATE_PARENT_INHERITANCE_ATTACK_APPLIED");
        std::io::stdout().flush().unwrap();
    }
    fn assert_native_loopback_denied(){
        let endpoint=std::env::var("ENDLESSVIBE_COMMAND_TEST_ADDRESS").unwrap()
            .parse::<std::net::SocketAddrV4>().unwrap();
        assert!(endpoint.ip().is_loopback());
        // Rust's std::net initializer asserts that WSAStartup succeeds. A
        // zero-capability LPAC can lose access even to Winsock initialization,
        // so measure that boundary directly without turning it into a panic.
        // The host proves that the same loopback endpoint is operational.
        let mut data:WSADATA=unsafe{zeroed()};
        let startup=unsafe{WSAStartup(0x0202,&mut data)};
        if startup!=0{
            assert!(matches!(startup,WSAEACCES|WSASYSCALLFAILURE),
                "Unexpected Winsock initialization failure in LPAC: {startup}");
            println!("LPAC_NETWORK_INIT_DENIED:{startup}");
            return;
        }
        struct Winsock;
        impl Drop for Winsock{fn drop(&mut self){unsafe{WSACleanup()};}}
        let _winsock=Winsock;
        let raw=unsafe{socket(AF_INET as i32,SOCK_STREAM,IPPROTO_TCP)};
        if raw==INVALID_SOCKET{
            let error=unsafe{WSAGetLastError()};
            assert_eq!(error,WSAEACCES,"LPAC socket creation failed for an unexpected reason");
            println!("LPAC_NETWORK_SOCKET_DENIED:{error}");
            return;
        }
        struct Socket(usize);
        impl Drop for Socket{fn drop(&mut self){unsafe{closesocket(self.0)};}}
        let socket=Socket(raw);
        let mut address:SOCKADDR_IN=unsafe{zeroed()};
        address.sin_family=AF_INET;
        address.sin_port=endpoint.port().to_be();
        address.sin_addr.S_un.S_addr=u32::from_ne_bytes(endpoint.ip().octets());
        let result=unsafe{connect(socket.0,&address as *const _ as _,size_of::<SOCKADDR_IN>() as i32)};
        let error=unsafe{WSAGetLastError()};
        assert_eq!(result,SOCKET_ERROR,"Offline LPAC connected to the host loopback listener");
        assert_eq!(error,WSAEACCES,
            "A live loopback endpoint must be denied by policy, not merely time out or refuse the connection");
        println!("LPAC_NETWORK_CONNECT_DENIED:{error}");
    }
    fn assert_security_edits_denied(path:&Path){
        const WRITE_DAC:u32=0x0004_0000;
        const WRITE_OWNER:u32=0x0008_0000;
        for (access,label) in [(WRITE_DAC,"WRITE_DAC"),(WRITE_OWNER,"WRITE_OWNER")]{
            let result=fs::OpenOptions::new().access_mode(access)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path);
            let error=result.expect_err(&format!(
                "LPAC obtained {label} on {} despite the per-SID deny",path.display()));
            assert_eq!(error.kind(),std::io::ErrorKind::PermissionDenied,
                "Security-edit probe failed for a reason other than policy: {} {label}: {error}",path.display());
        }
    }

    // Native child entry remains in the existing Rust test executable. Every
    // parent test below verifies concrete output/side effects from this entry.
    #[test]
    fn isolated_command_test_entry(){
        let Ok(mode)=std::env::var("ENDLESSVIBE_COMMAND_TEST_MODE")else{return};
        assert_current_lpac();
        let root=PathBuf::from(std::env::var_os("ENDLESSVIBE_COMMAND_TEST_ROOT").unwrap());
        match mode.as_str(){
            "boundaries"=>{
                let outside=PathBuf::from(std::env::var_os("ENDLESSVIBE_COMMAND_TEST_OUTSIDE").unwrap());
                let pid=std::env::var("ENDLESSVIBE_COMMAND_TEST_PARENT_PID").unwrap().parse().unwrap();
                assert_eq!(fs::read(root.join("input.txt")).unwrap(),b"isolated input");
                fs::write(root.join("result.txt"),b"isolated output").unwrap();
                assert_eq!(fs::read(&outside).unwrap_err().kind(),std::io::ErrorKind::PermissionDenied);
                assert_eq!(fs::OpenOptions::new().write(true).open(&outside).unwrap_err().kind(),
                    std::io::ErrorKind::PermissionDenied);
                assert_eq!(fs::read(root.join("aap-only.txt")).unwrap_err().kind(),std::io::ErrorKind::PermissionDenied);
                println!("LPAC_PRIVATE_FILES_DENIED");std::io::stdout().flush().unwrap();
                let parent=unsafe{OpenProcess(PROCESS_VM_READ,0,pid)};
                if !parent.is_null(){drop(unsafe{OwnedHandle::from_raw_handle(parent as _)});}
                assert!(parent.is_null(),"LPAC must not obtain the parent service process memory");
                println!("LPAC_PARENT_PROCESS_PROBE_PASSED");std::io::stdout().flush().unwrap();
                let mut available=0u32;
                let peek=unsafe{PeekNamedPipe(GetStdHandle(STD_INPUT_HANDLE),ptr::null_mut(),0,
                    ptr::null_mut(),&mut available,ptr::null_mut())};
                let error=std::io::Error::last_os_error();
                assert!(peek==0&&error.raw_os_error()==Some(ERROR_BROKEN_PIPE as i32),
                    "LPAC stdin must already have no writer before read_to_end: peek={peek} available={available} error={error}");
                let mut input=Vec::new();
                assert_eq!(std::io::stdin().read_to_end(&mut input).unwrap(),0);
                println!("LPAC_STDIN_EOF_VERIFIED");std::io::stdout().flush().unwrap();
                // Keep the independent network boundary last so a provider
                // initialization failure cannot obscure earlier observations.
                assert_native_loopback_denied();
                std::io::stdout().write_all("LPAC_STDOUT_中文\n".as_bytes()).unwrap();
                std::io::stdout().flush().unwrap();
                std::io::stderr().write_all(b"LPAC_STDERR\n").unwrap();
                std::io::stderr().flush().unwrap();
            },
            "tree"=>{
                let _middle=spawn_descendant("middle");
                println!("MIDDLE_PID:{}",_middle.id());
                std::io::stdout().flush().unwrap();
                loop{std::thread::sleep(Duration::from_secs(1));}
            },
            "middle"=>{
                let _leaf=spawn_descendant("leaf");
                println!("LEAF_PID:{}",_leaf.id());
                std::io::stdout().flush().unwrap();
                loop{std::thread::sleep(Duration::from_secs(1));}
            },
            "leaf"=>{
                fs::write(root.join("leaf-ready"),b"ready").unwrap();
                println!("LEAF_READY");
                std::io::stdout().flush().unwrap();
                loop{std::thread::sleep(Duration::from_secs(1));}
            },
            "orphan"=>{
                let _leaf=spawn_descendant("leaf");
                println!("LEAF_PID:{}",_leaf.id());
                std::io::stdout().flush().unwrap();
                let deadline=Instant::now()+Duration::from_secs(10);
                while !root.join("leaf-ready").exists()||!root.join("leaf-observed").exists(){
                    assert!(Instant::now()<deadline,"Native descendant never became ready");
                    std::thread::sleep(Duration::from_millis(10));
                }
            },
            "quiet"=>{
                println!("QUIET_READY");
                std::io::stdout().flush().unwrap();
                loop{std::thread::sleep(Duration::from_secs(1));}
            },
            "lease"=>{
                let project=PathBuf::from(std::env::var_os("ENDLESSVIBE_COMMAND_TEST_PROJECT").unwrap());
                let existing=project.join("existing.txt");
                assert_eq!(fs::read(&existing).unwrap(),b"existing source");
                fs::OpenOptions::new().append(true).open(&existing).unwrap()
                    .write_all(b" + sandbox edit").unwrap();
                let new_file=project.join("created.txt");
                let renamed=project.join("renamed.txt");
                fs::write(&new_file,b"create, rename and delete").unwrap();
                fs::rename(&new_file,&renamed).unwrap();
                fs::remove_file(&renamed).unwrap();
                let subdirectory=project.join("new-subdir");
                fs::create_dir(&subdirectory).unwrap();
                let generated=subdirectory.join("generated.txt");
                fs::write(&generated,b"child-created output").unwrap();
                let git=project.join(".git");
                let config=git.join("config");
                assert_eq!(fs::read(&config).unwrap(),b"private git configuration");
                // Execute and assert each operation before the next one, so
                // an unexpected delete cannot corrupt the rename evidence.
                assert_io_denied("append .git config",&config,
                    ||fs::OpenOptions::new().append(true).open(&config).map(|_|()));
                assert_io_denied("delete .git config",&config,||fs::remove_file(&config));
                assert_io_denied("rename .git config",&config,||fs::rename(&config,git.join("moved-config")));
                let new_git_file=git.join("new-file");
                assert_io_denied("create .git file",&new_git_file,||fs::write(&new_git_file,b"forbidden"));
                let new_git_directory=git.join("new-dir");
                assert_io_denied("create .git directory",&new_git_directory,||fs::create_dir(&new_git_directory));
                for path in [&project,&existing,&subdirectory,&generated,&git,&config]{
                    assert_security_edits_denied(path);
                }
                println!("LPAC_LEASE_RW_AND_GIT_RO_VERIFIED");
                println!("LPAC_OWNER_SECURITY_EDITS_DENIED");
                std::io::stdout().flush().unwrap();
            },
            "lease-revoked"=>{
                let project=PathBuf::from(std::env::var_os("ENDLESSVIBE_COMMAND_TEST_PROJECT").unwrap());
                for path in [project.join("existing.txt"),project.join("new-subdir/generated.txt")]{
                    assert_eq!(fs::read(path).unwrap_err().kind(),std::io::ErrorKind::PermissionDenied);
                }
                assert_eq!(fs::write(project.join("after-revoke.txt"),b"forbidden").unwrap_err().kind(),
                    std::io::ErrorKind::PermissionDenied);
                println!("LPAC_REVOKED_SID_CANNOT_ACCESS_PROJECT");
                std::io::stdout().flush().unwrap();
            },
            "sealed-tool-child"=>{
                assert_native_temporary_directory(&root);
                println!("LPAC_SEALED_TOOL_CHILD_EXECUTED");
                std::io::stdout().flush().unwrap();
            },
            "sealed-tools"=>{
                assert_native_temporary_directory(&root);
                let tools=root.join("tools");
                let data=tools.join("tool-data.txt");
                let include=tools.join("include");
                let header=include.join("sample.h");
                let empty=tools.join("empty");
                assert_eq!(fs::read(&data).unwrap(),b"immutable tool data");
                assert_eq!(fs::read(&header).unwrap(),b"immutable tool header");
                // The current executable and this ordinary child both live
                // inside the sealed tool tree; no host executable is used.
                assert!(spawn_descendant("sealed-tool-child").wait().unwrap().success());
                attempt_private_parent_acl_propagation(&root);
                for path in [&data,&header]{
                    assert_native_access_denied("append tool file",path,
                        ||fs::OpenOptions::new().append(true).open(path).map(|_|()));
                    assert_native_access_denied("overwrite tool file",path,||fs::write(path,b"forbidden"));
                    assert_native_access_denied("delete tool file",path,||fs::remove_file(path));
                    assert_native_access_denied("rename tool file",path,
                        ||fs::rename(path,root.join("stolen-tool-file")));
                }
                assert_native_access_denied("create tool file",&tools,
                    ||fs::write(tools.join("new-file"),b"forbidden"));
                assert_native_access_denied("create tool directory",&tools,
                    ||fs::create_dir(tools.join("new-directory")));
                assert_native_access_denied("delete empty tool directory",&empty,||fs::remove_dir(&empty));
                assert_native_access_denied("rename empty tool directory",&empty,
                    ||fs::rename(&empty,tools.join("renamed-empty")));
                assert_native_access_denied("rename tools root through its writable parent",&tools,
                    ||fs::rename(&tools,root.join("stolen-tools")));
                assert_native_access_denied("delete tools root through its writable parent",&tools,
                    ||fs::remove_dir(&tools));
                // Replacing an existing file by rename normally works in a
                // writable directory. Use an unmapped data file, so executable
                // image locks cannot make this negative test pass accidentally.
                let replacement=root.join("replacement-tool.txt");
                fs::write(&replacement,b"hostile replacement").unwrap();
                assert_native_access_denied("replace tool from writable profile",&data,
                    ||fs::rename(&replacement,&data));
                assert_eq!(fs::read(&replacement).unwrap(),b"hostile replacement");
                fs::remove_file(&replacement).unwrap();
                for path in [&root,&tools,&include,&empty,&data,&header]{
                    assert_security_edits_denied(path);
                }
                for path in [&root,&tools,&include]{
                    assert_native_access_denied("obtain FILE_DELETE_CHILD",path,||
                        fs::OpenOptions::new().access_mode(0x40)
                            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT)
                            .open(path).map(|_|()));
                }
                let temporary=root.join("Temp/seal-sibling");
                fs::create_dir(&temporary).unwrap();
                let scratch=temporary.join("before.txt");
                fs::write(&scratch,b"temporary output").unwrap();
                fs::OpenOptions::new().append(true).open(&scratch).unwrap().write_all(b" updated").unwrap();
                let renamed=temporary.join("after.txt");
                fs::rename(&scratch,&renamed).unwrap();
                assert_eq!(fs::read(&renamed).unwrap(),b"temporary output updated");
                fs::remove_file(&renamed).unwrap();
                let renamed_directory=root.join("Temp/seal-sibling-renamed");
                fs::rename(&temporary,&renamed_directory).unwrap();
                fs::remove_dir(&renamed_directory).unwrap();
                fs::write(root.join("seal-job-result.txt"),b"profile siblings remain writable").unwrap();
                assert_eq!(fs::read(&data).unwrap(),b"immutable tool data");
                assert_eq!(fs::read(&header).unwrap(),b"immutable tool header");
                println!("LPAC_PRIVATE_TOOL_SEAL_VERIFIED");
                println!("LPAC_PROFILE_TEMP_RW_VERIFIED");
                std::io::stdout().flush().unwrap();
            },
            _=>panic!("Unexpected isolated test mode"),
        }
    }

    #[test]
    fn native_argument_and_environment_encoding_are_unambiguous(){
        for(value,expected)in [("","\"\""),("a b","\"a b\""),
            ("tail\\","\"tail\\\\\""),("a\"b","\"a\\\"b\""),("中文","\"中文\"")]{
            let mut encoded=Vec::new();append_argument(&mut encoded,OsStr::new(value)).unwrap();
            assert_eq!(String::from_utf16(&encoded).unwrap(),expected);
        }
        let mut encoded=Vec::new();
        assert!(append_argument(&mut encoded,OsStr::new("bad\0arg")).is_err());
        let values=BTreeMap::from([(OsString::from("Path"),OsString::from("one")),
            (OsString::from("PATH"),OsString::from("two"))]);
        assert!(environment_block(&values).is_err());
        assert_eq!(environment_block(&BTreeMap::new()).unwrap(),vec![0,0]);
    }

    #[tokio::test]
    async fn native_lpac_denies_external_files_network_and_service_handles(){
        let(state,mut identity,mut spec)=fixture("boundaries");
        let outside=state.path().join("outside-canary.txt");
        crate::util::private_create(&outside,b"synthetic private canary").unwrap();
        assert_eq!(fs::read(&outside).unwrap(),b"synthetic private canary");
        let security=SECURITY_ATTRIBUTES{nLength:size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor:ptr::null_mut(),bInheritHandle:1};
        let canary=unsafe{CreateEventW(&security,1,0,ptr::null())};
        assert!(!canary.is_null(),"Create inheritable service event canary: {}",std::io::Error::last_os_error());
        let canary=unsafe{OwnedHandle::from_raw_handle(canary as _)};
        assert_eq!(unsafe{WaitForSingleObject(canary.as_raw_handle() as _,0)},WAIT_TIMEOUT);
        fs::write(identity.directory().join("input.txt"),b"isolated input").unwrap();
        write_aap_only_file(&identity.directory().join("aap-only.txt"));
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let address=listener.local_addr().unwrap();
        drop(TcpStream::connect_timeout(&address,Duration::from_secs(1)).unwrap());
        drop(listener.accept().unwrap());
        listener.set_nonblocking(true).unwrap();
        assert_eq!(listener.accept().unwrap_err().kind(),std::io::ErrorKind::WouldBlock);
        spec.environment.insert("ENDLESSVIBE_COMMAND_TEST_OUTSIDE".into(),outside.as_os_str().to_owned());
        spec.environment.insert("ENDLESSVIBE_COMMAND_TEST_ADDRESS".into(),address.to_string().into());
        spec.environment.insert("ENDLESSVIBE_COMMAND_TEST_PARENT_PID".into(),std::process::id().to_string().into());
        let mut running=spawn(&identity,&spec).unwrap();
        assert_service_handle_not_inherited(&running,&canary);
        let(mut out,mut err)=(Vec::new(),Vec::new());
        let outcome=running.run(&CancellationToken::new(),Duration::from_secs(15),|stream,bytes|{
            if stream=="stdout"{out.extend_from_slice(bytes);}else{err.extend_from_slice(bytes);}Ok(())
        }).await.unwrap();
        assert_eq!(unsafe{WaitForSingleObject(canary.as_raw_handle() as _,0)},WAIT_TIMEOUT,
            "Service-owned event handle leaked into the LPAC process");
        assert_eq!(outcome.exit_code,0,"out={} err={}",String::from_utf8_lossy(&out),String::from_utf8_lossy(&err));
        assert!(!outcome.cancelled&&!outcome.timed_out&&outcome.token_verified&&outcome.job_empty);
        assert!(String::from_utf8_lossy(&out).contains("LPAC_STDOUT_中文"));
        assert!(String::from_utf8_lossy(&err).contains("LPAC_STDERR"));
        assert!(String::from_utf8_lossy(&out).contains("LPAC_NETWORK_"));
        assert_eq!(listener.accept().unwrap_err().kind(),std::io::ErrorKind::WouldBlock,
            "Host listener received an unexpected connection during LPAC execution");
        assert_eq!(fs::read(identity.directory().join("result.txt")).unwrap(),b"isolated output");
        assert_eq!(fs::read(&outside).unwrap(),b"synthetic private canary");
        drop(running);
        identity.finish().unwrap();
        assert!(!identity.journal.exists());
    }

    #[tokio::test]
    async fn native_lpac_cancellation_kills_actual_children_and_grandchildren(){
        let(_state,mut identity,spec)=fixture("tree");
        let cancel=CancellationToken::new();
        let mut running=spawn(&identity,&spec).unwrap();
        let(mut output,mut middle,mut leaf)=(Vec::new(),None,None);
        let outcome=running.run(&cancel,Duration::from_secs(15),|_,bytes|{
            output.extend_from_slice(bytes);
            if middle.is_none(){if let Some(pid)=marker_pid(&output,"MIDDLE_PID:"){middle=Some(child_handle(pid));}}
            if leaf.is_none(){if let Some(pid)=marker_pid(&output,"LEAF_PID:"){leaf=Some(child_handle(pid));}}
            if middle.is_some()&&leaf.is_some()&&String::from_utf8_lossy(&output).contains("LEAF_READY"){cancel.cancel();}
            Ok(())
        }).await.unwrap();
        assert!(outcome.cancelled&&!outcome.timed_out&&outcome.job_empty,"{}",String::from_utf8_lossy(&output));
        assert_exited(middle.as_ref().expect("No actual child was observed"));
        assert_exited(leaf.as_ref().expect("No actual grandchild was observed"));
        drop(middle);drop(leaf);drop(running);identity.finish().unwrap();
    }

    #[tokio::test]
    async fn native_lpac_normal_exit_also_removes_orphaned_descendants(){
        let(_state,mut identity,spec)=fixture("orphan");
        let mut running=spawn(&identity,&spec).unwrap();
        let(mut output,mut leaf)=(Vec::new(),None);
        let outcome=running.run(&CancellationToken::new(),Duration::from_secs(15),|_,bytes|{
            output.extend_from_slice(bytes);
            if leaf.is_none(){if let Some(pid)=marker_pid(&output,"LEAF_PID:"){
                leaf=Some(child_handle(pid));
                fs::write(identity.directory().join("leaf-observed"),b"observed").unwrap();
            }}
            Ok(())
        }).await.unwrap();
        assert_eq!(outcome.exit_code,0,"{}",String::from_utf8_lossy(&output));
        assert!(!outcome.cancelled&&!outcome.timed_out&&outcome.job_empty);
        assert_exited(leaf.as_ref().expect("No actual descendant was observed"));
        drop(leaf);drop(running);identity.finish().unwrap();
    }

    #[tokio::test]
    async fn native_lpac_timeout_streams_quiet_output_and_flushes_before_exit(){
        let(_state,mut identity,spec)=fixture("quiet");
        let mut running=spawn(&identity,&spec).unwrap();
        let(mut output,mut flushed_ready)=(Vec::new(),false);
        let outcome=running.run(&CancellationToken::new(),Duration::from_secs(3),|_,bytes|{
            if bytes.is_empty()&&String::from_utf8_lossy(&output).contains("QUIET_READY"){flushed_ready=true;}
            output.extend_from_slice(bytes);Ok(())
        }).await.unwrap();
        assert!(outcome.timed_out&&!outcome.cancelled&&outcome.job_empty,"{}",String::from_utf8_lossy(&output));
        assert!(flushed_ready,"Quiet command output was not flushed while the child was alive");
        drop(running);identity.finish().unwrap();
    }

    #[test]
    fn native_lpac_drop_kills_suspended_process_before_identity_cleanup(){
        let(_state,mut identity,spec)=fixture("quiet");
        let running=spawn(&identity,&spec).unwrap();
        let handle=child_handle(running.id());
        assert!(identity.active.load(Ordering::Acquire));
        drop(running);
        assert_exited(&handle);
        drop(handle);
        assert!(!identity.active.load(Ordering::Acquire));
        identity.finish().unwrap();
    }

    #[tokio::test]
    async fn native_lpac_lease_keeps_git_readonly_blocks_owner_edits_and_revokes_new_files(){
        use crate::tools::windows_acl::{Access,Grant,Lease};
        let(state,mut identity,mut spec)=fixture("lease");
        let project=state.path().join("project");
        let git=project.join(".git");
        fs::create_dir_all(git.join("objects")).unwrap();
        fs::write(project.join("existing.txt"),b"existing source").unwrap();
        fs::write(git.join("config"),b"private git configuration").unwrap();
        let journal=state.path().join("acl-journal");
        let mut lease=Lease::acquire(&journal,identity.sid_string(),&[
            Grant{path:project.clone(),access:Access::ReadWrite},
            Grant{path:git.clone(),access:Access::ReadExecute},
        ],&[state.path().join("appcontainer-profiles")]).unwrap();
        spec.cwd=project.clone();
        spec.environment.insert("ENDLESSVIBE_COMMAND_TEST_PROJECT".into(),project.as_os_str().to_owned());
        let mut running=spawn(&identity,&spec).unwrap();
        let mut output=Vec::new();
        let result=running.run(&CancellationToken::new(),Duration::from_secs(15),|_,bytes|{
            output.extend_from_slice(bytes);Ok(())
        }).await;
        drop(running);
        if !identity.job_empty(){lease.defer_cleanup();identity.defer_cleanup();}
        assert!(identity.job_empty(),"ACL revocation must wait for the complete Job Object to exit");
        let outcome=result.unwrap();
        assert_eq!(outcome.exit_code,0,"{}",String::from_utf8_lossy(&output));
        assert!(outcome.job_empty&&!outcome.cancelled&&!outcome.timed_out);
        assert!(String::from_utf8_lossy(&output).contains("LPAC_OWNER_SECURITY_EDITS_DENIED"));
        assert_eq!(fs::read(project.join("existing.txt")).unwrap(),b"existing source + sandbox edit");
        assert_eq!(fs::read(project.join("new-subdir/generated.txt")).unwrap(),b"child-created output");
        assert_eq!(fs::read(git.join("config")).unwrap(),b"private git configuration");
        assert!(!git.join("new-file").exists()&&!git.join("moved-config").exists());
        lease.finish().unwrap();
        assert_eq!(fs::read_dir(&journal).unwrap().count(),0);

        // Reuse this exact SID only inside the test to prove actual access was
        // removed from both old objects and objects created by the LPAC child.
        spec.environment.insert("ENDLESSVIBE_COMMAND_TEST_MODE".into(),"lease-revoked".into());
        spec.cwd=identity.directory().to_owned();
        let mut running=spawn(&identity,&spec).unwrap();
        let mut revoked_output=Vec::new();
        let result=running.run(&CancellationToken::new(),Duration::from_secs(15),|_,bytes|{
            revoked_output.extend_from_slice(bytes);Ok(())
        }).await;
        drop(running);
        if !identity.job_empty(){lease.defer_cleanup();identity.defer_cleanup();}
        assert!(identity.job_empty());
        let outcome=result.unwrap();
        assert_eq!(outcome.exit_code,0,"{}",String::from_utf8_lossy(&revoked_output));
        assert!(outcome.job_empty&&!outcome.cancelled&&!outcome.timed_out);
        assert!(String::from_utf8_lossy(&revoked_output).contains("LPAC_REVOKED_SID_CANNOT_ACCESS_PROJECT"));
        assert!(!project.join("after-revoke.txt").exists());
        fs::write(project.join("host-can-still-write.txt"),b"host access retained").unwrap();
        drop(lease);
        identity.finish().unwrap();
    }

    #[tokio::test]
    async fn native_lpac_private_tools_are_readonly_and_profile_temp_remains_writable(){
        use crate::tools::windows_acl::{private_acl_snapshot,seal_private_tools};
        let(_state,mut identity,mut spec)=fixture("sealed-tools");
        let wrong_profile=match seal_private_tools(_state.path(),identity.sid_string()){
            Ok(_)=>panic!("Private tool sealing accepted an unrelated host directory"),
            Err(error)=>error,
        };
        assert!(wrong_profile.to_string().contains("exact native profile"),"{wrong_profile:#}");
        let tools=identity.directory().join("tools");
        fs::create_dir_all(tools.join("bin")).unwrap();
        fs::create_dir(tools.join("include")).unwrap();
        fs::create_dir(tools.join("empty")).unwrap();
        fs::write(tools.join("tool-data.txt"),b"immutable tool data").unwrap();
        fs::write(tools.join("include/sample.h"),b"immutable tool header").unwrap();
        let executable=tools.join("bin/command-test.exe");
        let mut source=fs::File::open(&spec.program).unwrap();
        let mut destination=fs::OpenOptions::new().write(true).create_new(true).open(&executable).unwrap();
        std::io::copy(&mut source,&mut destination).unwrap();
        destination.sync_all().unwrap();
        drop(destination);drop(source);
        spec.program=executable;
        let seal=seal_private_tools(identity.directory(),identity.sid_string()).unwrap();
        let snapshot=|stage:&str|{
            for(label,path)in [("package",identity.directory().parent().unwrap().to_owned()),
                ("ac",identity.directory().to_owned()),("tools",tools.clone()),
                ("data",tools.join("tool-data.txt"))]{
                println!("LPAC_TOOL_ACL_SNAPSHOT:{stage}:{label}:{}",private_acl_snapshot(&path).unwrap());
            }
        };
        snapshot("sealed");
        let mut running=spawn(&identity,&spec).unwrap();
        snapshot("suspended");
        let mut output=Vec::new();
        let result=running.run(&CancellationToken::new(),Duration::from_secs(15),|_,bytes|{
            output.extend_from_slice(bytes);Ok(())
        }).await;
        drop(running);
        snapshot("finished");
        assert!(identity.job_empty(),"Private tool pins must outlive the entire native process tree");
        let outcome=result.unwrap();
        assert_eq!(outcome.exit_code,0,"{}",String::from_utf8_lossy(&output));
        assert!(outcome.token_verified&&outcome.job_empty&&!outcome.cancelled&&!outcome.timed_out);
        for marker in ["LPAC_SEALED_TOOL_CHILD_EXECUTED","LPAC_PRIVATE_TOOL_SEAL_VERIFIED","LPAC_PROFILE_TEMP_RW_VERIFIED"]{
            assert!(String::from_utf8_lossy(&output).contains(marker),"Missing {marker}: {}",String::from_utf8_lossy(&output));
        }
        assert_eq!(String::from_utf8_lossy(&output).matches("LPAC_NATIVE_TEMP_VERIFIED:GetTempPathW:").count(),2,
            "Native private temporary paths were not verified in both ordinary process generations: {}",String::from_utf8_lossy(&output));
        for line in String::from_utf8_lossy(&output).lines().filter(|line|
            line.contains("LPAC_NATIVE_TEMP_")||line.contains("LPAC_PRIVATE_PARENT_")){
            println!("{line}");
        }
        assert!(String::from_utf8_lossy(&output).contains("LPAC_PRIVATE_PARENT_WRITE_DAC_DENIED")
            ||String::from_utf8_lossy(&output).contains("LPAC_PRIVATE_PARENT_INHERITANCE_ATTACK_APPLIED"),
            "Private package parent propagation was not tested: {}",String::from_utf8_lossy(&output));
        assert_eq!(fs::read(tools.join("tool-data.txt")).unwrap(),b"immutable tool data");
        assert_eq!(fs::read(tools.join("include/sample.h")).unwrap(),b"immutable tool header");
        assert_eq!(fs::read(identity.directory().join("seal-job-result.txt")).unwrap(),b"profile siblings remain writable");
        assert!(!identity.directory().join("stolen-tools").exists());
        assert!(!tools.join("new-file").exists()&&!tools.join("new-directory").exists());
        drop(seal);
        let directory=identity.directory().to_owned();
        identity.finish().unwrap();
        assert!(!directory.exists(),"Native profile cleanup left a sealed tool copy behind");
        assert!(!identity.journal.exists());
    }
}
