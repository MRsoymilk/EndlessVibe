// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: windows_trusted_worker.rs
// Description: Stage only the already-running EndlessVibe executable into
// an AppContainer with bounded bytes and verified file-handle identity.
//
// No API accepts an arbitrary Project binary, script, command or EXE path.

use anyhow::{bail,Context,Result};
use sha2::{Digest,Sha256};
use std::{
    fs::{File,OpenOptions},
    io::{Read,Seek,SeekFrom,Write},
    os::windows::{fs::{OpenOptionsExt,MetadataExt},ffi::OsStringExt},
    ffi::OsString,
    path::{Path,PathBuf},
};

pub const MAX_WORKER_EXECUTABLE_BYTES:u64=128*1024*1024;

#[derive(Debug,Clone,PartialEq,Eq)]
struct RuntimeStamp{module:String,sha256:String,size:u64}
const IO_PAGE_BYTES:usize=64*1024;
const OPEN_REPARSE_POINT:u32=0x0020_0000;

/// Snapshot of the trusted current process executable, never user code.
///
/// Only a new, private AppContainer directory is a valid staging target.
/// verify() must pass both before CreateProcessW and after child exit.
#[derive(Debug)]
pub(crate) struct TrustedWorker {
    path:PathBuf,
    sha256:String,
    bytes:u64,
    imports:Vec<String>,
    runtime_stamps:Vec<RuntimeStamp>,
}
impl TrustedWorker{
    pub(crate) fn stage_current(private_dir:&Path)->Result<Self>{
        let source=std::env::current_exe()
            .context("Resolve already-running EndlessVibe executable")?;
        let mut original=open_pinned_regular(&source,false)
            .context("Open trusted process executable without following links")?;
        let size=original.metadata()?.len();
        if size==0||size>MAX_WORKER_EXECUTABLE_BYTES{
            bail!("Trusted Windows Worker EXE exceeds the 128 MiB staging limit");
        }
        let pinned=crate::security::paths::Root::open(private_dir)
            .context("Pin private AppContainer staging directory")?;
        pinned.unchanged_root()?;

        // The profile is unique, and create_new rejects any pre-existing
        // executable, including symlinks. Never modify the original Project.
        let path=private_dir.join("fixed-worker.exe");
        let mut staged=OpenOptions::new().write(true).create_new(true)
            .custom_flags(OPEN_REPARSE_POINT)
            .open(&path).context("Create private, isolated Worker executable")?;
        let copied={
            let mut bounded=(&mut original).take(MAX_WORKER_EXECUTABLE_BYTES+1);
            std::io::copy(&mut bounded,&mut staged)
                .context("Copy trusted Worker through pinned source handle")?
        };
        if copied!=size{
            bail!("Worker executable changed while staging or exceeded byte budget");
        }
        staged.flush()?;
        staged.sync_all()?;
        if !crate::platform::single_link(&staged){
            bail!("Staged Worker has unexpected file links");
        }
        drop(staged);
        pinned.unchanged_root()?;

        // Hash the same pinned source handle used for the copy, not a newly
        // resolved pathname which could refer to another executable.
        let (digest,read)=sha256_file(&mut original)?;
        if read!=size{bail!("Trusted executable changed after staging");}
        // The PE module list is constrained independently of the byte hash.
        // No caller-supplied DLL directories or dynamic delay imports.
        let audit=inspect_staged_imports(&path,size)?;
        let runtime_stamps=attest_msvc_runtimes(&audit.msvc_runtime_modules)?;
        let worker=Self{path,sha256:digest,bytes:size,imports:audit.modules,runtime_stamps};
        worker.verify()?;
        Ok(worker)
    }
    pub(crate) fn path(&self)->&Path{&self.path}
    pub(crate) fn sha256(&self)->&str{&self.sha256}
    pub(crate) fn bytes(&self)->u64{self.bytes}
    pub(crate) fn imports(&self)->&[String]{&self.imports}
    pub(crate) fn msvc_runtimes(&self)->Vec<&str>{
        self.runtime_stamps.iter().map(|item|item.module.as_str()).collect()
    }
    pub(crate) fn verify(&self)->Result<()>{
        let mut staged=open_pinned_regular(&self.path,true)
            .context("Read staged Worker with reparse and link checks")?;
        let (actual,count)=sha256_file(&mut staged)?;
        if count!=self.bytes||actual!=self.sha256{
            bail!("Staged Windows Worker executable integrity mismatch");
        }
        let pe=inspect_imports_handle(&mut staged,self.bytes)?;
        if pe.modules!=self.imports{
            bail!("Staged Windows Worker dependency footprint changed after verification");
        }
        let now=attest_msvc_runtimes(&pe.msvc_runtime_modules)?;
        if now!=self.runtime_stamps{
            bail!("Windows System32 MSVC runtime changed during Worker verification");
        }
        Ok(())
    }
}
fn inspect_imports_handle(file:&mut File,bytes:u64)
    ->Result<crate::tools::windows_pe::PeImports>{
    if bytes>MAX_WORKER_EXECUTABLE_BYTES{
        bail!("PE import audit exceeds trusted executable byte limit");
    }
    file.seek(SeekFrom::Start(0))?;
    let mut data=Vec::with_capacity(bytes as usize);
    (&mut *file).take(bytes+1).read_to_end(&mut data)?;
    if data.len() as u64!=bytes{
        bail!("PE import audit observed a changed executable byte count");
    }
    crate::tools::windows_pe::inspect_pe_imports(&data)
}
fn inspect_staged_imports(path:&Path,bytes:u64)
    ->Result<crate::tools::windows_pe::PeImports>{
    let mut file=open_pinned_regular(path,true)?;
    let inspected=inspect_imports_handle(&mut file,bytes)?;
    if inspected.machine!=0x8664||inspected.has_delay_load{
        bail!("Only dependency-audited AMD64 fixed worker binaries are supported");
    }
    Ok(inspected)
}
// Get the actual Windows system directory from Kernel32, never PATH,
// current working directory or a caller-supplied SystemRoot variable.
pub(crate) fn system32_directory()->Result<PathBuf>{
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    let mut chars=[0u16;512];
    let len=unsafe{GetSystemDirectoryW(chars.as_mut_ptr(),chars.len() as u32)} as usize;
    if len==0||len>=chars.len(){
        bail!("Cannot safely discover the Windows System32 directory");
    }
    let path=PathBuf::from(OsString::from_wide(&chars[..len]));
    let metadata=std::fs::symlink_metadata(&path)
        .context("Inspect canonical Windows System32 directory")?;
    if !path.is_absolute()||!metadata.is_dir()||
       metadata.file_attributes() & 0x400!=0{
        bail!("Windows system directory is not a regular absolute directory");
    }
    Ok(path)
}
fn attest_msvc_runtimes(names:&[String])->Result<Vec<RuntimeStamp>>{
    if names.is_empty(){return Ok(Vec::new());}
    let dir=system32_directory()?;
    let mut stamps=Vec::with_capacity(names.len());
    for name in names{
        // The PE parser permits only known MSVC redistributable basenames.
        // Validate again here so a future parser change cannot become a path
        // traversal or caller-selected DLL staging primitive.
        if !matches!(name.as_str(),
            "vcruntime140.dll"|"vcruntime140_1.dll"|"msvcp140.dll"|"concrt140.dll"){
            bail!("Unsupported MSVC runtime import");
        }
        let path=dir.join(name);
        let metadata=std::fs::symlink_metadata(&path)
            .with_context(||format!("Required System32 MSVC runtime is missing: {name}"))?;
        if !metadata.is_file()||metadata.file_attributes() & 0x400!=0{
            bail!("MSVC runtime in System32 is not a regular non-reparse DLL");
        }
        let mut file=open_pinned_regular(&path,false)
            .context("Pin approved MSVC runtime from Windows System32")?;
        let (sha256,size)=sha256_file(&mut file)?;
        if size==0{bail!("MSVC runtime in System32 was empty");}
        stamps.push(RuntimeStamp{module:name.clone(),sha256,size});
    }
    Ok(stamps)
}
fn open_pinned_regular(path:&Path,require_single_link:bool)->Result<File>{
    let mut opts=OpenOptions::new();
    opts.read(true).custom_flags(OPEN_REPARSE_POINT);
    let file=opts.open(path)?;
    let md=std::fs::symlink_metadata(path)?;
    if !file.metadata()?.is_file()||md.file_type().is_symlink(){
        bail!("Trusted Worker must be a regular, non-reparse executable file");
    }
    // The running Cargo executable may be hardlinked to its deps artifact.
    // That is safe as source because its loaded image and pinned handle are
    // the trust anchor. The private staged *copy* must be single-link.
    if require_single_link&&!crate::platform::single_link(&file){
        bail!("Staged Worker must be a single-link, non-reparse regular file");
    }
    Ok(file)
}
fn sha256_file(file:&mut File)->Result<(String,u64)>{
    file.seek(SeekFrom::Start(0))?;
    let mut hasher=Sha256::new();
    let mut page=[0u8;IO_PAGE_BYTES];
    let mut count=0u64;
    loop{
        let n=file.read(&mut page)?;
        if n==0{break;}
        count=count.checked_add(n as u64).context("Worker byte count overflow")?;
        if count>MAX_WORKER_EXECUTABLE_BYTES{
            bail!("Trusted executable exceeds 128 MiB staging limit");
        }
        hasher.update(&page[..n]);
    }
    Ok((format!("{:x}",hasher.finalize()),count))
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::fs;
    #[test]
    fn msvc_runtimes_must_be_attested_from_system32(){
        let sys=system32_directory().unwrap();
        assert!(sys.is_absolute());
        let checked=attest_msvc_runtimes(&["vcruntime140.dll".to_owned()]).unwrap();
        assert_eq!(checked.len(),1);
        assert_eq!(checked[0].module,"vcruntime140.dll");
        assert!(checked[0].size>0);
        assert_eq!(checked[0].sha256.len(),64);
        for name in ["../vcruntime140.dll","unknown.dll","VCRUNTIME140.dll","winhttp.dll"]{
            assert!(attest_msvc_runtimes(&[name.into()]).is_err(),
                "Non-approved runtime entry {name:?} was accepted");
        }
    }
    #[test]
    fn stage_only_current_exe_and_detect_tampered_copy(){
        let temp=tempfile::tempdir().unwrap();
        let trusted=TrustedWorker::stage_current(temp.path()).unwrap();
        assert_eq!(trusted.path(),temp.path().join("fixed-worker.exe"));
        assert!(trusted.bytes()>0);
        assert_eq!(trusted.sha256().len(),64);
        trusted.verify().unwrap();
        assert_eq!(fs::read(std::env::current_exe().unwrap()).unwrap(),
                   fs::read(trusted.path()).unwrap());
        let mut file=OpenOptions::new().append(true).open(trusted.path()).unwrap();
        file.write_all(b"\nmalicious change\n").unwrap();
        file.sync_all().unwrap();
        assert!(trusted.verify().is_err(),"Tampered Worker copy must fail closed");
    }
    #[test]
    fn stage_never_accepts_an_arbitrary_executable_or_hardlink(){
        let temp=tempfile::tempdir().unwrap();
        let arbitrary=temp.path().join("attacker.exe");
        fs::write(&arbitrary,b"untrusted").unwrap();
        // There is intentionally no source argument to stage_current().
        let worker=TrustedWorker::stage_current(temp.path()).unwrap();
        assert_ne!(fs::read(worker.path()).unwrap(),fs::read(&arbitrary).unwrap());
        assert!(TrustedWorker::stage_current(temp.path()).is_err(),
                "Cannot overwrite or reuse an existing staged EXE");
        let hardlink=temp.path().join("hardlinked.exe");
        if fs::hard_link(&arbitrary,&hardlink).is_ok(){
            assert!(open_pinned_regular(&arbitrary,true).is_err(),
                    "Hardlinked executable aliases are prohibited");
        }
    }
}
