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
    os::windows::fs::OpenOptionsExt,
    path::{Path,PathBuf},
};

pub const MAX_WORKER_EXECUTABLE_BYTES:u64=128*1024*1024;
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
        let worker=Self{path,sha256:digest,bytes:size};
        worker.verify()?;
        Ok(worker)
    }
    pub(crate) fn path(&self)->&Path{&self.path}
    pub(crate) fn sha256(&self)->&str{&self.sha256}
    pub(crate) fn bytes(&self)->u64{self.bytes}
    pub(crate) fn verify(&self)->Result<()>{
        let mut staged=open_pinned_regular(&self.path,true)
            .context("Read staged Worker with reparse and link checks")?;
        let (actual,count)=sha256_file(&mut staged)?;
        if count!=self.bytes||actual!=self.sha256{
            bail!("Staged Windows Worker executable integrity mismatch");
        }
        Ok(())
    }
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
