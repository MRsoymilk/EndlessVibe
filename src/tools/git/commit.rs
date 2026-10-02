use crate::{runtime::Runtime,tools::{tasks,types::CommitArgs},workspace::Project};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{fs::File,io::Write,os::unix::fs::MetadataExt};
use super::{recovery::{acquire_index_lock,create_journal},repo::{args,good,head,oid,preflight,repo,Environment},review::{index_records,preview}};

pub async fn commit(rt:&Runtime,w:&Project,a:CommitArgs)->Result<Value>{
    w.commit_allowed()?;tasks::validate_context(&a.task_id,&a.stage)?;preflight(rt,w).await?;
    if a.paths.is_empty(){bail!("git_commit requires explicit nonempty paths");}
    if a.message.trim().is_empty()||a.message.len()>4096||a.message.contains('\0'){bail!("Invalid commit message");}
    let gitdir=repo(w)?;let mut lock=acquire_index_lock(&gitdir)?;
    let branch_raw=good(rt,w,args(&["symbolic-ref","--quiet","HEAD"]),&Environment::default(),None).await.context("Commits require a checked-out branch; detached HEAD is refused")?;
    let branch=String::from_utf8(branch_raw)?.trim().to_owned();if !branch.starts_with("refs/heads/"){bail!("Unexpected HEAD reference");}
    let p=preview(rt,w,a.paths).await?;
    if p.head!=a.expected_head||p.review!=a.expected_diff_sha256{return Err(crate::error::coded_details("GIT_CONFLICT",true,"HEAD or reviewed file contents changed; call git_diff again",json!({"expected_head":a.expected_head,"actual_head":p.head,"expected_diff_sha256":a.expected_diff_sha256,"actual_diff_sha256":p.review})));}if p.diff.is_empty(){bail!("Nothing to commit");}
    let mut check=args(&["diff","--cached","--name-only","-z","--no-ext-diff","--no-textconv","--ignore-submodules=all","--"]);check.extend(p.paths.clone());
    if !good(rt,w,check,&Environment::default(),None).await?.is_empty(){return Err(crate::error::coded_details("STAGED_CONFLICT",false,"a selected path already contains staged user work; unstage/review it yourself before retrying",json!({"paths":p.paths})));}
    for c in &p.changes{if let Some(bytes)=&c.bytes{let id=oid(good(rt,w,args(&["hash-object","-w","--no-filters","--stdin"]),&Environment::default(),Some(bytes.clone())).await?)?;if id!=c.oid{bail!("Git object format changed during commit");}}}
    let temp_env=Environment{index:Some(p.index.clone()),objects:None};
    let tree=oid(good(rt,w,args(&["write-tree"]),&temp_env,None).await?)?;
    let mut create=vec!["commit-tree".into(),tree];if p.head!="UNBORN"{create.push("-p".into());create.push(p.head.clone());}
    let commit_id=oid(good(rt,w,create,&Environment::default(),Some(format!("{}\n",a.message.trim()).into_bytes())).await?)?;
    let merged=p._temp.path().join("merged-index");let index_path=gitdir.join("index");
    if index_path.exists(){let md=std::fs::symlink_metadata(&index_path)?;if !md.is_file()||md.file_type().is_symlink()||md.nlink()!=1||md.len()>64*1024*1024{bail!("Unsafe/oversized Git index");}std::fs::copy(&index_path,&merged)?;}else{let env=Environment{index:Some(merged.clone()),objects:None};good(rt,w,args(&["read-tree","--empty"]),&env,None).await?;}
    let merged_env=Environment{index:Some(merged.clone()),objects:None};good(rt,w,args(&["update-index","-z","--index-info"]),&merged_env,Some(index_records(&p.changes,commit_id.len()))).await?;
    let index_bytes=std::fs::read(&merged)?;lock.file.write_all(&index_bytes)?;lock.file.sync_all()?;
    let current_branch=String::from_utf8(good(rt,w,args(&["symbolic-ref","--quiet","HEAD"]),&Environment::default(),None).await?)?;if current_branch.trim()!=branch{bail!("Branch changed during commit");}
    let journal=create_journal(rt,w,&branch,&p.head,&commit_id,&lock.path,&index_bytes)?;
    let expected=if p.head=="UNBORN"{"0".repeat(commit_id.len())}else{p.head.clone()};
    lock.keep=true;
    let update=good(rt,w,vec!["update-ref".into(),"-m".into(),format!("EndlessVibe: {}",a.message.lines().next().unwrap_or("commit")),branch.clone(),commit_id.clone(),expected],&Environment::default(),None).await;
    if let Err(e)=update{
        let actual=head(rt,w).await;
        if actual.as_ref().is_ok_and(|h|h==&commit_id){}else if actual.as_ref().is_ok_and(|h|h==&p.head){lock.keep=false;let _=std::fs::remove_file(&journal);return Err(e).context("Branch compare-and-swap failed; working tree and real index are unchanged");}else{return Err(crate::error::coded_details("GIT_UPDATE_AMBIGUOUS",false,format!("index.lock and {} were retained for manual recovery; do not reset the repository: {e}",journal.display()),json!({"journal":journal})));}
    }
    lock.keep=true;
    if let Err(e)=std::fs::rename(&lock.path,&index_path){return Err(crate::error::coded_details("COMMIT_PARTIALLY_PUBLISHED",false,format!("HEAD is now {commit_id}; index.lock and {} were retained for recovery: {e}",journal.display()),json!({"commit":commit_id,"journal":journal})));}
    lock.published=true;let durability_warning=File::open(&gitdir).and_then(|d|d.sync_all()).is_err();let _=std::fs::remove_file(&journal);let operation_diff=p.diff.clone();
    let checkpoint_warning=if let (Some(task),Some(stage))=(a.task_id.as_deref(),a.stage.as_deref()){tasks::record_commit(&rt.db,&w.workspace_id,&w.config.id,task,stage,&commit_id).err().map(|error|{tracing::warn!(error=%error,commit=%commit_id,"Commit succeeded but checkpoint persistence failed");"Commit succeeded but task checkpoint could not be persisted"})}else{None};
    Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"commit":commit_id,"branch":branch,"paths":p.paths,"task_id":a.task_id,"stage":a.stage,"checkpoint":if checkpoint_warning.is_none()&&a.task_id.is_some(){"committed"}else if a.task_id.is_some(){"warning"}else{"not_requested"},"checkpoint_warning":checkpoint_warning,"pushed":false,"unrelated_staging_preserved":true,"working_tree_not_rewritten":true,"durability_warning":durability_warning,"_operation_diff":operation_diff}))
}
