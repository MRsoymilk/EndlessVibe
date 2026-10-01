use crate::{runtime::Runtime,util,workspace::Project};
use anyhow::Result;
use serde_json::json;
use std::{fs::{File,OpenOptions},os::unix::fs::OpenOptionsExt,path::{Path,PathBuf}};

pub(super) struct IndexLock{pub(super) path:PathBuf,pub(super) file:File,pub(super) keep:bool,pub(super) published:bool}
impl Drop for IndexLock{fn drop(&mut self){if !self.keep&&!self.published{let _=std::fs::remove_file(&self.path);}}}

pub(super) fn acquire_index_lock(gitdir:&Path)->Result<IndexLock>{
    let path=gitdir.join("index.lock");
    let file=OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path).map_err(|_|crate::error::coded("GIT_BUSY",true,"index.lock exists; never remove another process's lock automatically"))?;
    Ok(IndexLock{path,file,keep:false,published:false})
}

pub(super) fn create_journal(rt:&Runtime,w:&Project,branch:&str,old_head:&str,new_head:&str,index_lock:&Path,index_bytes:&[u8])->Result<PathBuf>{
    let journal=rt.config.security.data_dir.join("git-journal").join(format!("{}.json",util::random_secret()?));
    util::private_create(&journal,&serde_json::to_vec_pretty(&json!({
        "workspace":w.workspace_id,
        "project":w.config.id,
        "branch":branch,
        "old_head":old_head,
        "new_head":new_head,
        "index_lock":index_lock,
        "index_sha256":util::digest(index_bytes),
        "note":"If HEAD=new_head and index.lock has the recorded hash, publish that lock file as index. Otherwise inspect manually; never reset or discard work automatically."
    }))?)?;
    Ok(journal)
}
