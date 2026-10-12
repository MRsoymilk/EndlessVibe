// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: windows_loaded_modules.rs
// Description: Inspect actual Windows image mappings for the fixed Worker.
//
// Toolhelp is an independent parent-process snapshot, NOT Authenticode
// verification or a guarantee about future LoadLibrary calls.

use anyhow::{bail,Context,Result};
use std::{
    collections::BTreeSet,
    ffi::{OsString,c_void},
    fs::{self,File,OpenOptions},
    mem::{size_of,zeroed,MaybeUninit},
    os::windows::{ffi::OsStringExt,fs::{MetadataExt,OpenOptionsExt},io::{AsRawHandle,FromRawHandle,OwnedHandle}},
    path::{Path,PathBuf},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{GetLastError,ERROR_BAD_LENGTH,ERROR_NO_MORE_FILES,INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot,Module32FirstW,Module32NextW,MODULEENTRY32W,
            TH32CS_SNAPMODULE,TH32CS_SNAPMODULE32,
        },
        Threading::{OpenProcess,PROCESS_QUERY_INFORMATION,PROCESS_VM_READ},
    },
};
use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION,GetFileInformationByHandle,GetFinalPathNameByHandleW,FILE_FLAG_OPEN_REPARSE_POINT,FILE_ATTRIBUTE_REPARSE_POINT,FILE_NAME_NORMALIZED,VOLUME_NAME_DOS};

const MAX_LOADED_MODULES:usize=256;
const SNAPSHOT_RETRIES:usize=8;
const REPARSE_POINT:u32=0x400;

#[derive(Clone,Debug)]
struct LoadedImage {name:String,path:PathBuf,base:usize}
#[derive(Debug)]
pub(crate) struct ModuleAudit {
    pub(crate) basenames:Vec<String>,
    pub(crate) system32_modules:usize,
    pub(crate) executable_verified:bool,
    pub(crate) backing_mapping_verified:bool,
    pinned:std::collections::BTreeMap<String,PinnedImage>,
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct FileStamp{
    volume:u32,
    file_index:u64,
    bytes:u64,
    last_write:u64,
}
#[derive(Debug)]
struct PinnedImage{handle:File,stamp:FileStamp}

fn nul_terminated(chars:&[u16])->Result<OsString>{
    let end=chars.iter().position(|&c|c==0)
        .context("Windows loaded-module path was truncated")?;
    if end==0{bail!("Windows loaded-module path is empty");}
    Ok(OsString::from_wide(&chars[..end]))
}
fn snapshot(pid:u32)->Result<Vec<LoadedImage>>{
    if pid==0{bail!("Cannot enumerate modules of PID zero");}
    let mut handle=None;
    for attempt in 0..SNAPSHOT_RETRIES{
        let raw=unsafe{CreateToolhelp32Snapshot(TH32CS_SNAPMODULE|TH32CS_SNAPMODULE32,pid)};
        if raw!=INVALID_HANDLE_VALUE{
            handle=Some(unsafe{OwnedHandle::from_raw_handle(raw as _)});
            break;
        }
        let err=unsafe{GetLastError()};
        if err!=ERROR_BAD_LENGTH{
            return Err(std::io::Error::from_raw_os_error(err as i32))
                .context("Enumerate actual Windows process modules");
        }
        if attempt+1<SNAPSHOT_RETRIES{std::thread::sleep(Duration::from_millis(15));}
    }
    let snapshot=handle.context("Unstable Windows module list exceeded retry limit")?;
    let mut entry:MODULEENTRY32W=unsafe{zeroed()};
    entry.dwSize=size_of::<MODULEENTRY32W>() as u32;
    if unsafe{Module32FirstW(snapshot.as_raw_handle() as _,&mut entry)}==0{
        return Err(std::io::Error::last_os_error()).context("Read first loaded module");
    }
    let mut modules=Vec::new();
    loop{
        if modules.len()==MAX_LOADED_MODULES{bail!("Windows process has too many mapped modules");}
        let name=nul_terminated(&entry.szModule)?.to_string_lossy().to_ascii_lowercase();
        let path=PathBuf::from(nul_terminated(&entry.szExePath)?);
        if !path.is_absolute()||!name.is_ascii(){
            bail!("Loaded module path is not an absolute Windows pathname");
        }
        let stem=path.file_name().and_then(|s|s.to_str())
            .context("Loaded module has no Unicode basename")?;
        if !stem.eq_ignore_ascii_case(&name){
            bail!("Loaded module basename and image path disagree");
        }
        if entry.modBaseAddr.is_null(){bail!("Windows returned a null mapped-module base");}
        modules.push(LoadedImage{name,path,base:entry.modBaseAddr as usize});
        entry.dwSize=size_of::<MODULEENTRY32W>() as u32;
        if unsafe{Module32NextW(snapshot.as_raw_handle() as _,&mut entry)}==0{
            let code=unsafe{GetLastError()};
            if code!=ERROR_NO_MORE_FILES{
                return Err(std::io::Error::from_raw_os_error(code as i32))
                    .context("Enumerate remaining Windows process modules");
            }
            break;
        }
    }
    Ok(modules)
}
fn comparable_path(path:&Path)->Result<String>{
    let canonical=fs::canonicalize(path)
        .with_context(||format!("Resolve actual loaded-module path: {}",path.display()))?;
    let metadata=fs::symlink_metadata(path)?;
    if !metadata.is_file()||metadata.file_attributes()&REPARSE_POINT!=0{
        bail!("Loaded image is not a regular, non-reparse file");
    }
    Ok(canonical.to_string_lossy().to_lowercase())
}
fn classify(modules:&[LoadedImage],expected_executable:&Path,system32:&Path)
    ->Result<ModuleAudit>{
    if modules.is_empty()||modules.len()>MAX_LOADED_MODULES{
        bail!("No bounded set of loaded Windows modules is available");
    }
    let worker=comparable_path(expected_executable)?;
    let allowed_dir=fs::canonicalize(system32)?
        .to_string_lossy().to_lowercase();
    let mut seen=BTreeSet::new();
    let mut own=0usize;
    let mut windows_count=0usize;
    for module in modules{
        if !seen.insert(module.name.clone()){
            bail!("Duplicate loaded module basename is not allowed");
        }
        let path=comparable_path(&module.path)?;
        if path==worker{
            if module.name!="fixed-worker.exe" && module.name!="endlessvibe.exe"
                && !module.name.ends_with(".exe"){
                bail!("Unexpected executable module name");
            }
            own+=1;
            continue;
        }
        let parent=fs::canonicalize(module.path.parent()
            .context("Mapped module has no containing directory")?)?
            .to_string_lossy().to_lowercase();
        if parent!=allowed_dir||!module.name.ends_with(".dll"){
            bail!("AppContainer loaded an image outside trusted System32: {}",
                  module.name);
        }
        if path==worker{bail!("Conflicting Worker and system DLL identity");}
        windows_count+=1;
    }
    if own!=1||windows_count==0{
        bail!("Expected one fixed Worker EXE and at least one System32 DLL mapping");
    }
    Ok(ModuleAudit{
        basenames:seen.into_iter().collect(),
        system32_modules:windows_count,
        executable_verified:true,
        backing_mapping_verified:false,
        pinned:std::collections::BTreeMap::new(),
    })
}
// GetMappedFileNameW returns the *mapped section's* NT device name for a
// base address, independently of Toolhelp's module path string. The caller
// compares the two via the current QueryDosDeviceW drive mapping. This is
// not a filesystem file-ID or Authenticode signature attestation.
#[link(name="kernel32")]
unsafe extern "system" {
    fn K32GetMappedFileNameW(process:*mut c_void,base:*mut c_void,
        output:*mut u16,length:u32)->u32;
    fn QueryDosDeviceW(drive:*const u16,output:*mut u16,length:u32)->u32;
}
fn device_prefix(letter:u8)->Result<String>{
    if !letter.is_ascii_alphabetic(){
        bail!("Mapped image must use a local Windows drive");
    }
    let drive=[letter.to_ascii_uppercase() as u16,b':' as u16,0u16];
    let mut buffer=[0u16;32768];
    let len=unsafe{QueryDosDeviceW(drive.as_ptr(),buffer.as_mut_ptr(),buffer.len() as u32)};
    if len==0||len as usize>=buffer.len(){
        return Err(std::io::Error::last_os_error()).context("Resolve Windows drive NT-device mapping");
    }
    let zero=buffer[..len as usize].iter().position(|v|*v==0)
        .context("Windows device mapping is not NUL-terminated")?;
    let path=String::from_utf16(&buffer[..zero])
        .context("Windows device mapping contains invalid UTF-16")?;
    if !path.to_ascii_lowercase().starts_with(r"\device\"){
        bail!("Unexpected non-local Windows device mapping");
    }
    Ok(path)
}
fn expected_device_path(path:&Path,drives:&mut std::collections::BTreeMap<u8,String>)
    ->Result<String>{
    let canonical=fs::canonicalize(path)?;
    let canonical=canonical.to_str().context("Mapped module path is not Unicode")?;
    let ordinary=canonical.strip_prefix(r"\\?\").unwrap_or(canonical);
    let bytes=ordinary.as_bytes();
    if bytes.len()<4||!bytes[0].is_ascii_alphabetic()||bytes[1]!=b':'
        ||bytes[2]!=b'\\'{
        bail!("Expected a fully qualified local Windows drive path");
    }
    let letter=bytes[0].to_ascii_uppercase();
    if let std::collections::btree_map::Entry::Vacant(entry)=drives.entry(letter){
        entry.insert(device_prefix(letter)?);
    }
    let prefix=drives.get(&letter).context("Missing Windows drive device mapping")?;
    Ok(format!("{}{}",prefix,&ordinary[2..]).to_lowercase())
}
fn mapped_section_name(
    process:windows_sys::Win32::Foundation::HANDLE,base:usize,
)->Result<String>{
    if base==0{bail!("Mapped Windows image base is null");}
    let mut characters=[0u16;32768];
    let count=unsafe{
        K32GetMappedFileNameW(process,base as *mut c_void,
            characters.as_mut_ptr(),characters.len() as u32)
    };
    if count==0{
        return Err(std::io::Error::last_os_error()).context("Inspect Windows mapped section backing path");
    }
    if count as usize>=characters.len()-1{
        bail!("Windows mapped image path exceeds bounded output buffer");
    }
    let name=String::from_utf16(&characters[..count as usize])
        .context("Windows mapped image path is invalid UTF-16")?;
    if !name.to_lowercase().starts_with(r"\device\"){
        bail!("Windows mapped image did not resolve to an NT device path");
    }
    Ok(name.to_lowercase())
}
fn verify_mapped_sections(pid:u32,modules:&[LoadedImage])->Result<()>{
    let raw=unsafe{OpenProcess(PROCESS_QUERY_INFORMATION|PROCESS_VM_READ,0,pid)};
    if raw.is_null(){
        return Err(std::io::Error::last_os_error()).context("Open AppContainer for mapped-section auditing");
    }
    let process=unsafe{OwnedHandle::from_raw_handle(raw as _)};
    let mut drives=std::collections::BTreeMap::new();
    for module in modules{
        let expected=expected_device_path(&module.path,&mut drives)?;
        let actual=mapped_section_name(process.as_raw_handle() as _,module.base)
            .with_context(||format!("Audit memory-mapped module {}",module.name))?;
        if actual!=expected{
            bail!("Mapped image backing section and Toolhelp path disagree for {}",module.name);
        }
    }
    Ok(())
}

// Pin every actually enumerated image file with a non-following Windows file
// handle. Compare stable volume/file indexes, length and last-write FILETIME
// across the two gated observations. The section's own file ID is not
// exposed here by documented Win32 APIs; never claim it is proven.
fn file_stamp(handle:&File)->Result<FileStamp>{
    let mut result=MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    if unsafe{GetFileInformationByHandle(handle.as_raw_handle() as _,result.as_mut_ptr())}==0{
        return Err(std::io::Error::last_os_error()).context("Inspect pinned Windows file identity");
    }
    let info=unsafe{result.assume_init()};
    let index=((info.nFileIndexHigh as u64)<<32)|info.nFileIndexLow as u64;
    if index==0||info.nNumberOfLinks==0
        ||info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT!=0{
        bail!("Pinned Windows image has an ambiguous ID or reparse point");
    }
    let metadata=handle.metadata()?;
    if !metadata.is_file(){bail!("Pinned Windows image is not a regular file");}
    Ok(FileStamp{
        volume:info.dwVolumeSerialNumber,
        file_index:index,
        bytes:((info.nFileSizeHigh as u64)<<32)|info.nFileSizeLow as u64,
        last_write:((info.ftLastWriteTime.dwHighDateTime as u64)<<32)
            |info.ftLastWriteTime.dwLowDateTime as u64,
    })
}
fn opened_path(handle:&File)->Result<String>{
    let mut chars=[0u16;32768];
    let count=unsafe{GetFinalPathNameByHandleW(
        handle.as_raw_handle() as _,chars.as_mut_ptr(),chars.len() as u32,
        FILE_NAME_NORMALIZED|VOLUME_NAME_DOS,
    )};
    if count==0{
        return Err(std::io::Error::last_os_error()).context("Resolve pinned Windows image handle");
    }
    if count as usize>=chars.len()-1{
        bail!("Pinned Windows image path exceeds maximum allowed UTF-16 buffer");
    }
    Ok(String::from_utf16(&chars[..count as usize])
        .context("Invalid UTF-16 in pinned Windows file handle path")?.to_lowercase())
}
fn pin_file(path:&Path)->Result<PinnedImage>{
    let mut options=OpenOptions::new();
    options.read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let handle=options.open(path).context("Pin loaded Windows image file without following reparse points")?;
    let identity=file_stamp(&handle)?;
    let canonical=fs::canonicalize(path)
        .context("Resolve module path separately from opened file handle")?
        .to_string_lossy().to_lowercase();
    if opened_path(&handle)?!=canonical{
        bail!("Pinned loaded image handle and canonical path do not match");
    }
    Ok(PinnedImage{handle,stamp:identity})
}
fn pin_loaded_images(modules:&[LoadedImage])
    ->Result<std::collections::BTreeMap<String,PinnedImage>>{
    let mut result=std::collections::BTreeMap::new();
    for module in modules{
        let pinned=pin_file(&module.path)
            .with_context(||format!("Pin enumerated Windows module {}",module.name))?;
        if result.insert(module.name.clone(),pinned).is_some(){
            bail!("Duplicate Windows module while pinning image identity");
        }
    }
    Ok(result)
}
fn ensure_pinned_images_stable(before:&ModuleAudit,after:&ModuleAudit)->Result<()>{
    if before.pinned.is_empty()||after.pinned.is_empty(){
        bail!("Missing pinned Windows image handles from live snapshots");
    }
    // The earlier handles stay open until after the second snapshot and
    // completion report. A path swap then produces a different file index.
    for (name,first) in &before.pinned{
        if file_stamp(&first.handle)?!=first.stamp{
            bail!("Pinned Windows module was modified after initial identity check: {name}");
        }
        if let Some(second)=after.pinned.get(name){
            if first.stamp!=second.stamp{
                bail!("Windows module file identity changed between snapshots: {name}");
            }
        }
    }
    for (name,second) in &after.pinned{
        if file_stamp(&second.handle)?!=second.stamp{
            bail!("Windows module changed during final identity check: {name}");
        }
    }
    Ok(())
}

pub(crate) fn audit(pid:u32,expected_executable:&Path)->Result<ModuleAudit>{
    let sys=crate::tools::windows_trusted_worker::system32_directory()?;
    let modules=snapshot(pid)?;
    let mut reviewed=classify(&modules,expected_executable,&sys)?;
    verify_mapped_sections(pid,&modules)?;
    reviewed.backing_mapping_verified=true;
    reviewed.pinned=pin_loaded_images(&modules)?;
    Ok(reviewed)
}

/// Two short fixed-operation barriers keep the trusted lowbox Worker alive
/// while the privileged parent independently inspects live image mappings.
/// This is not a general code-execution scheduler.
pub(crate) struct LoaderGate{
    root:crate::security::paths::Root,
    executable:PathBuf,
    phase:u8,
    before:Option<ModuleAudit>,
    after:Option<ModuleAudit>,
}
impl LoaderGate{
    pub(crate) fn new(private_root:&Path,executable:&Path)->Result<Self>{
        let root=crate::security::paths::Root::open(private_root)?;
        root.unchanged_root()?;
        Ok(Self{root,executable:executable.to_owned(),phase:0,
            before:None,after:None})
    }
    pub(crate) fn observe(&mut self,pid:u32)->Result<()>{
        if self.phase==0{
            if let Some(ready)=self.root.read_optional("outputs/worker-loader-ready.ack",16)?{
                if ready!=b"ready"{bail!("Invalid AppContainer loaded-module ready message");}
                let checked=audit(pid,&self.executable)?;
                self.root.replace("worker-loader-allow.ack",b"allow",true,false)?;
                self.before=Some(checked);
                self.phase=1;
            }
        }
        if self.phase==1{
            if let Some(done)=self.root.read_optional("outputs/worker-loader-done.ack",16)?{
                if done!=b"done"{bail!("Invalid AppContainer loaded-module done message");}
                let checked=audit(pid,&self.executable)?;
                self.root.replace("worker-loader-release.ack",b"release",true,false)?;
                self.after=Some(checked);
                self.phase=2;
            }
        }
        Ok(())
    }
    pub(crate) fn summary(&self)->Result<serde_json::Value>{
        if self.phase!=2{bail!("AppContainer exited before both live module checks");}
        let before=self.before.as_ref().context("Missing pre-execution module check")?;
        let after=self.after.as_ref().context("Missing post-operation module check")?;
        ensure_pinned_images_stable(before,after)?;
        Ok(serde_json::json!({
            "toolhelp_parent_verified":true,
            "startup_snapshot_modules":before.basenames,
            "completed_snapshot_modules":after.basenames,
            "pre_system32_count":before.system32_modules,
            "post_system32_count":after.system32_modules,
            "expected_worker_identity":before.executable_verified&&after.executable_verified,
            "pinned_file_ids_stable_across_snapshots":true,
            "pinned_file_count":before.pinned.len(),
            "loaded_section_file_id_proven":false,
            "mapped_section_backing_paths_verified":before.backing_mapping_verified&&after.backing_mapping_verified,
            "outside_system32_module_rejected":true,
            "snapshot_count":2,
            "future_or_transient_module_loads_proven":false,
            "authenticode_signatures_verified":false,
        }))
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    #[test]
    fn actual_current_windows_process_modules_are_system_origin(){
        // Native Toolhelp enumeration and canonical file identity, without a
        // staged AppContainer process. The gated worker test covers the child.
        let executable=std::env::current_exe().unwrap();
        let modules=snapshot(unsafe{GetCurrentProcessId()}).unwrap();
        assert!(!modules.is_empty());
        // The current unit-test binary is named differently to fixed-worker.
        let summary=classify(&modules,&executable,
            &crate::tools::windows_trusted_worker::system32_directory().unwrap()).unwrap();
        assert!(summary.executable_verified);
        assert!(summary.system32_modules>0);
        assert!(summary.basenames.iter().any(|n|n.ends_with(".exe")));
    }
    #[test]
    fn mapped_backing_name_matches_toolhelp_module_and_rejects_mismatch(){
        let pid=unsafe{GetCurrentProcessId()};
        let modules=snapshot(pid).unwrap();
        assert!(modules.len()>1);
        verify_mapped_sections(pid,&modules).unwrap();
        let reviewed=audit(pid,&std::env::current_exe().unwrap()).unwrap();
        assert!(reviewed.backing_mapping_verified);
        let mut invalid=modules.clone();
        invalid[0].base=usize::MAX;
        assert!(verify_mapped_sections(pid,&invalid).is_err(),
            "An invalid loaded image base address must fail closed");
        let mut wrong=modules;
        let other=wrong.iter().find(|m|m.path!=wrong[0].path).unwrap().path.clone();
        wrong[0].path=other;
        assert!(verify_mapped_sections(pid,&wrong).is_err(),
            "A Toolhelp image path that differs from mapped section identity must be refused");
    }
    #[test]
    fn pinned_file_stamp_detects_mutation_and_wrong_file_identity(){
        use std::io::Write;
        let dir=tempfile::tempdir().unwrap();
        let module=dir.path().join("system.dll");
        let other=dir.path().join("other.dll");
        fs::write(&module,b"original").unwrap();
        fs::write(&other,b"unrelated").unwrap();
        let pin=pin_file(&module).unwrap();
        assert_eq!(file_stamp(&pin.handle).unwrap(),pin.stamp);
        assert_eq!(pin_file(&module).unwrap().stamp,pin.stamp);
        assert_ne!(pin_file(&other).unwrap().stamp,pin.stamp,
            "Different files cannot share an accepted Windows volume/file-index identity");
        let mut second=std::collections::BTreeMap::new();
        second.insert("system.dll".into(),pin_file(&module).unwrap());
        let mut first=std::collections::BTreeMap::new();
        first.insert("system.dll".into(),pin);
        let audit=|pinned|ModuleAudit{
            basenames:vec!["system.dll".into()],
            system32_modules:1,
            executable_verified:true,
            backing_mapping_verified:true,
            pinned,
        };
        let pre=audit(first);
        let post=audit(second);
        ensure_pinned_images_stable(&pre,&post).unwrap();

        // A writer retaining the same file index still changes the pinned
        // metadata and must invalidate the pre-execution approval.
        let mut writer=fs::OpenOptions::new().append(true).open(&module).unwrap();
        writer.write_all(b"modified after approval").unwrap();
        writer.sync_all().unwrap();
        assert!(ensure_pinned_images_stable(&pre,&post).is_err(),
            "Modified loaded file must invalidate a previously approved snapshot");
    }
    #[test]
    fn pinned_module_identity_disallows_swapped_handles(){
        let dir=tempfile::tempdir().unwrap();
        let a=dir.path().join("one.dll");
        let b=dir.path().join("two.dll");
        fs::write(&a,b"one").unwrap();
        fs::write(&b,b"two").unwrap();
        let make=|path:&Path|{
            let mut ids=std::collections::BTreeMap::new();
            ids.insert("same.dll".into(),pin_file(path).unwrap());
            ModuleAudit{
                basenames:vec!["same.dll".into()],
                system32_modules:1,
                executable_verified:true,
                backing_mapping_verified:true,
                pinned:ids,
            }
        };
        let before=make(&a);
        let after=make(&b);
        assert!(ensure_pinned_images_stable(&before,&after).is_err(),
            "A replaced same-name module must not be accepted by file identity");
    }
    #[test]
    fn untrusted_image_paths_and_duplicate_mappings_fail_closed(){
        let expected=std::env::current_exe().unwrap();
        let mut images=snapshot(unsafe{GetCurrentProcessId()}).unwrap();
        let system=crate::tools::windows_trusted_worker::system32_directory().unwrap();
        assert!(classify(&images,&expected,&system).is_ok());

        let repeated=images[0].clone();
        images.push(repeated);
        assert!(classify(&images,&expected,&system).is_err(),
            "Duplicate image basename must never pass loader audit");
        images.pop();

        let temporary=tempfile::tempdir().unwrap();
        let untrusted=temporary.path().join("evil.dll");
        fs::write(&untrusted,b"untrusted DLL").unwrap();
        images.push(LoadedImage{name:"evil.dll".into(),path:untrusted,base:0});
        assert!(classify(&images,&expected,&system).is_err(),
            "File outside System32 must be rejected");
    }
    #[test]
    fn parent_gate_rejects_forged_readiness_and_incomplete_snapshots(){
        let temporary=tempfile::tempdir().unwrap();
        fs::create_dir(temporary.path().join("outputs")).unwrap();
        let executable=std::env::current_exe().unwrap();
        let mut gate=LoaderGate::new(temporary.path(),&executable).unwrap();
        assert!(gate.summary().is_err(),
            "A gate cannot succeed with no module snapshots");
        let root=crate::security::paths::Root::open(temporary.path()).unwrap();
        root.replace("outputs/worker-loader-ready.ack",b"bogus",true,false).unwrap();
        assert!(gate.observe(unsafe{GetCurrentProcessId()}).is_err(),
            "Forged child readiness must not open the module gate");
        assert!(root.read_optional("worker-loader-allow.ack",8).unwrap().is_none());
    }

    #[test]
    fn missing_or_untrusted_loaded_images_fail_closed(){
        assert!(snapshot(0).is_err());
        assert!(classify(&[],Path::new("C:\\missing.exe"),Path::new("C:\\Windows\\System32")).is_err());
        let current=std::env::current_exe().unwrap();
        let name=current.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
        let fake=LoadedImage{name,path:current,base:0};
        assert!(classify(&[fake],Path::new("C:\\missing.exe"),
            &crate::tools::windows_trusted_worker::system32_directory().unwrap()).is_err());
    }
}
