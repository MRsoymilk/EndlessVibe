use crate::store::Store;
use anyhow::{Context,Result};
use serde_json::{json,Value};
use std::{fs,path::Path};

const MAX_SCAN_NODES:usize=20_000;

#[derive(Clone,Copy,Debug,Default)]
struct SizeInfo{bytes:u64,nodes:u64,truncated:bool}
impl SizeInfo{
    fn merge(&mut self,other:Self){self.bytes=self.bytes.saturating_add(other.bytes);self.nodes=self.nodes.saturating_add(other.nodes);self.truncated|=other.truncated;}
    fn json(self)->Value{json!({"bytes":self.bytes,"nodes":self.nodes,"truncated":self.truncated})}
}

fn scan(path:&Path,budget:&mut usize)->Result<SizeInfo>{
    if *budget==0{return Ok(SizeInfo{truncated:true,..Default::default()});}
    let metadata=match fs::symlink_metadata(path){Ok(value)=>value,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(SizeInfo::default()),Err(error)=>return Err(error).with_context(||format!("Read storage metadata {}",path.display()))};
    *budget-=1;
    if metadata.file_type().is_symlink(){return Ok(SizeInfo{nodes:1,..Default::default()});}
    if metadata.is_file(){return Ok(SizeInfo{bytes:metadata.len(),nodes:1,truncated:false});}
    if !metadata.is_dir(){return Ok(SizeInfo{nodes:1,..Default::default()});}
    let mut total=SizeInfo{nodes:1,..Default::default()};
    for entry in fs::read_dir(path).with_context(||format!("Read storage directory {}",path.display()))?{
        if *budget==0{total.truncated=true;break;}
        total.merge(scan(&entry?.path(),budget)?);
    }
    Ok(total)
}

fn scan_one(path:&Path)->Result<Value>{let mut budget=MAX_SCAN_NODES;Ok(scan(path,&mut budget)?.json())}
fn file_bytes(path:&Path)->Result<u64>{match fs::symlink_metadata(path){Ok(md)if md.is_file()&&!md.file_type().is_symlink()=>Ok(md.len()),Ok(_)=>Ok(0),Err(error)if error.kind()==std::io::ErrorKind::NotFound=>Ok(0),Err(error)=>Err(error).with_context(||format!("Read storage file {}",path.display()))}}

pub fn snapshot(data_dir:&Path,db:&Store,retained_jobs:usize)->Result<Value>{
    let main=data_dir.join("state.sqlite3");
    let wal=data_dir.join("state.sqlite3-wal");
    let shm=data_dir.join("state.sqlite3-shm");
    let main_bytes=file_bytes(&main)?;let wal_bytes=file_bytes(&wal)?;let shm_bytes=file_bytes(&shm)?;
    Ok(json!({
        "generated_at":crate::util::now(),
        "database":{
            "main_bytes":main_bytes,
            "wal_bytes":wal_bytes,
            "shm_bytes":shm_bytes,
            "physical_bytes":main_bytes.saturating_add(wal_bytes).saturating_add(shm_bytes),
            "logical":db.storage_metadata(retained_jobs)?
        },
        "state":scan_one(data_dir)?,
        "directories":{
            "exec_cache":scan_one(&data_dir.join("exec-cache"))?,
            "backups":scan_one(&data_dir.join("backups"))?,
            "git_journal":scan_one(&data_dir.join("git-journal"))?,
            "tmp":scan_one(&data_dir.join("tmp"))?
        },
        "scan":{"max_nodes_per_path":MAX_SCAN_NODES,"follows_symlinks":false}
    }))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn scan_is_bounded_and_does_not_follow_symlinks(){let d=tempfile::tempdir().unwrap();std::fs::write(d.path().join("a"),b"1234").unwrap();std::os::unix::fs::symlink(d.path().join("a"),d.path().join("link")).unwrap();let mut budget=100;let value=scan(d.path(),&mut budget).unwrap();assert_eq!(value.bytes,4);assert_eq!(value.nodes,3);assert!(!value.truncated);}
}
