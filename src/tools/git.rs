//! Git operations use literal pathspecs, no hooks/signing/filter execution, a private
//! preview object store, and a temporary index. Commit preserves unrelated staging.
use crate::{runtime::Runtime,tools::{process,types::*},util,workspace::Project};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{collections::BTreeSet,fs::{File,OpenOptions},io::Write,os::unix::fs::{MetadataExt,OpenOptionsExt},path::PathBuf};
use tokio::process::Command;

#[derive(Clone,Default)]struct Environment{index:Option<PathBuf>,objects:Option<PathBuf>}
struct Change{path:String,bytes:Option<Vec<u8>>,mode:String,oid:String}
struct Preview{_temp:tempfile::TempDir,index:PathBuf,head:String,paths:Vec<String>,diff:String,review:String,changes:Vec<Change>}
struct IndexLock{path:PathBuf,file:File,keep:bool,published:bool}
impl Drop for IndexLock{fn drop(&mut self){if !self.keep&&!self.published{let _=std::fs::remove_file(&self.path);}}}
fn repo(w:&Project)->Result<PathBuf>{
    w.root.unchanged_root()?;let dir=w.root.path.join(".git");let md=std::fs::symlink_metadata(&dir).context("Project is not a standalone Git repository; linked worktrees and subdirectory repositories are not supported by this release")?;
    if !md.is_dir()||md.file_type().is_symlink(){bail!(".git must be a real directory (linked worktrees/bare repositories are not supported)");}
    if dir.to_string_lossy().contains([':', '\n', '\r']){bail!("Git repository paths containing ':' or newlines are not supported");}
    Ok(dir)
}
// Parse a private copy with git-config only: no includes, helpers, filters or repository discovery.
// This is a single-owner safeguard, not isolation from a hostile process running as the same UID.
async fn preflight(rt:&Runtime,w:&Project)->Result<()> {
    let dir=repo(w)?;
    for part in ["config","HEAD","index","objects","refs","packed-refs","shallow"] {
        let path=dir.join(part);if let Ok(md)=std::fs::symlink_metadata(&path){if md.file_type().is_symlink(){bail!("Git metadata symlinks are not supported: {part}");}}
    }
    for part in ["commondir","objects/info/alternates","objects/info/http-alternates"]{if dir.join(part).exists(){bail!("External/shared Git metadata is not supported: {part}");}}
    let path=dir.join("config");let mut data=Vec::new();
    if path.exists(){use std::io::Read;let file=OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK).open(&path)?;let md=file.metadata()?;if !md.is_file()||md.nlink()!=1||md.len()>262144{bail!("Unsafe/oversized Git config");}file.take(262145).read_to_end(&mut data)?;if data.len()>262144{bail!("Oversized Git config");}}
    let mut temp=tempfile::Builder::new().prefix("git-config-").tempfile_in(rt.config.security.data_dir.join("tmp"))?;temp.write_all(&data)?;temp.flush()?;
    let mut cmd=Command::new(&rt.config.git.executable);process::clean_environment(&mut cmd,"/usr/bin:/bin");cmd.current_dir(rt.config.security.data_dir.join("empty-home")).env("HOME",rt.config.security.data_dir.join("empty-home")).env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL","/dev/null").args(["config","--null","--no-includes","--file"]).arg(temp.path()).arg("--list");
    let result=process::capture(cmd,None,524288,5).await?;if result.code!=Some(0){bail!("Git config cannot be safely parsed");}
    for line in result.stdout.split(|b|*b==0).filter(|b|!b.is_empty()){
        let key=String::from_utf8_lossy(line.split(|b|*b==b'\n').next().unwrap_or_default()).to_ascii_lowercase();
        if ["include.","includeif.","filter.","diff.","merge.","alias.","credential.","gpg.","lfs."].iter().any(|p|key.starts_with(*p)) || (key.starts_with("remote.")&&key.ends_with(".promisor")) || (key.starts_with("extensions.")&&key!="extensions.objectformat") || matches!(key.as_str(),"core.fsmonitor"|"core.sparsecheckout"|"core.sparsecheckoutcone") {bail!("Repository config key '{key}' needs manual review; helper/filter/include/sparse/partial-clone configurations are not supported by the dedicated Git tools. Existing config was not changed.");}
    }
    Ok(())
}
fn command(rt:&Runtime,w:&Project,env:&Environment)->Result<Command>{
    let gitdir=repo(w)?;let mut c=Command::new(&rt.config.git.executable);process::clean_environment(&mut c,"/usr/bin:/bin");
    c.current_dir(&w.root.path).env("HOME",rt.config.security.data_dir.join("empty-home")).env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_ATTR_NOSYSTEM","1").env("GIT_LITERAL_PATHSPECS","1").env("GIT_TERMINAL_PROMPT","0").env("GIT_OPTIONAL_LOCKS","0").env("GIT_NO_REPLACE_OBJECTS","1").env("GIT_NO_LAZY_FETCH","1").env("GIT_AUTHOR_NAME",&rt.config.git.author_name).env("GIT_AUTHOR_EMAIL",&rt.config.git.author_email).env("GIT_COMMITTER_NAME",&rt.config.git.author_name).env("GIT_COMMITTER_EMAIL",&rt.config.git.author_email);
    c.arg("--no-pager").arg("--git-dir").arg(&gitdir).arg("--work-tree").arg(&w.root.path);
    for setting in ["core.hooksPath=/dev/null","core.fsmonitor=false","commit.gpgSign=false","tag.gpgSign=false","core.attributesFile=/dev/null","diff.external=","core.quotePath=false","submodule.recurse=false","protocol.allow=never","core.gitProxy=/bin/false","core.sshCommand=/bin/false"]{c.arg("-c").arg(setting);}
    if let Some(index)=&env.index{c.env("GIT_INDEX_FILE",index);}if let Some(objects)=&env.objects{c.env("GIT_OBJECT_DIRECTORY",objects).env("GIT_ALTERNATE_OBJECT_DIRECTORIES",gitdir.join("objects"));}Ok(c)
}
async fn run(rt:&Runtime,w:&Project,args:Vec<String>,env:&Environment,input:Option<Vec<u8>>)->Result<process::Captured>{let mut cmd=command(rt,w,env)?;cmd.args(args);process::capture(cmd,input,rt.config.limits.max_output_bytes.max(4*1024*1024),20).await}
fn args(values:&[&str])->Vec<String>{values.iter().map(|s|(*s).to_owned()).collect()}
async fn good(rt:&Runtime,w:&Project,values:Vec<String>,env:&Environment,input:Option<Vec<u8>>)->Result<Vec<u8>>{let out=run(rt,w,values,env,input).await?;if out.code!=Some(0){bail!("Git failed: {}",util::bounded_text(&String::from_utf8_lossy(&out.stderr),2048));}Ok(out.stdout)}
async fn head(rt:&Runtime,w:&Project)->Result<String>{let out=run(rt,w,args(&["rev-parse","--verify","--quiet","HEAD"]),&Environment::default(),None).await?;match out.code{Some(0)=>Ok(String::from_utf8(out.stdout)?.trim().to_owned()),Some(1)=>Ok("UNBORN".into()),_=>bail!("Cannot determine repository HEAD")}}
fn oid(bytes:Vec<u8>)->Result<String>{let value=String::from_utf8(bytes)?.trim().to_owned();if !matches!(value.len(),40|64)||!value.bytes().all(|c|c.is_ascii_hexdigit()){bail!("Git returned an invalid object ID");}Ok(value)}
fn parse_status(bytes:&[u8])->Result<Vec<Value>>{
    let mut values=bytes.split(|b|*b==0).filter(|b|!b.is_empty());let mut out=Vec::new();
    while let Some(value)=values.next(){if value.len()<4{bail!("Invalid Git status record");}let x=value[0] as char;let y=value[1] as char;let path=String::from_utf8(value[3..].to_vec())?;let original=if matches!(x,'R'|'C')||matches!(y,'R'|'C'){values.next().map(|v|String::from_utf8(v.to_vec())).transpose()?}else{None};
        if crate::security::paths::relative(&path,false).is_ok(){out.push(json!({"index":x.to_string(),"worktree":y.to_string(),"path":path,"original_path":original.filter(|p|crate::security::paths::relative(p,false).is_ok())}));}}
    Ok(out)
}
pub async fn status(rt:&Runtime,w:&Project)->Result<Value>{preflight(rt,w).await?;let raw=good(rt,w,args(&["status","--porcelain=v1","-z","--untracked-files=all","--ignore-submodules=all"]),&Environment::default(),None).await?;Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"head":head(rt,w).await?,"entries":parse_status(&raw)?,"sensitive_paths_omitted":true}))}
pub async fn log(rt:&Runtime,w:&Project,a:LogArgs)->Result<Value>{preflight(rt,w).await?;if a.limit==0||a.limit>100{bail!("limit must be 1..100");}if head(rt,w).await?=="UNBORN"{return Ok(json!({"commits":[]}));}let values=vec!["log".into(),format!("-{}",a.limit),"--format=%H%x00%an%x00%aI%x00%s%x00".into(),"--no-decorate".into()];let raw=good(rt,w,values,&Environment::default(),None).await?;let mut out=Vec::new();let fields=raw.split(|b|*b==0).collect::<Vec<_>>();for chunk in fields.chunks(4){if chunk.len()==4{out.push(json!({"commit":String::from_utf8_lossy(chunk[0]).trim(),"author":String::from_utf8_lossy(chunk[1]),"date":String::from_utf8_lossy(chunk[2]),"subject":String::from_utf8_lossy(chunk[3])}));}}Ok(json!({"commits":out}))}
async fn selected(rt:&Runtime,w:&Project,paths:Vec<String>)->Result<Vec<String>>{
    let mut paths:BTreeSet<String>=paths.into_iter().collect();if paths.is_empty(){let s=status(rt,w).await?;if let Some(entries)=s["entries"].as_array(){for e in entries{if let Some(p)=e["path"].as_str(){paths.insert(p.to_owned());}if let Some(p)=e["original_path"].as_str(){paths.insert(p.to_owned());}}}}
    if paths.len()>64{bail!("Select at most 64 exact files per review/commit");}
    for p in &paths{crate::security::paths::relative(p,false)?;if let Ok(md)=w.root.metadata(p){if !md.is_file()||md.nlink()!=1{bail!("Git review accepts regular files only; symlinks, hardlinks, submodules and directories are refused");}}}
    Ok(paths.into_iter().collect())
}
fn index_records(changes:&[Change],hash_len:usize)->Vec<u8>{let mut data=Vec::new();for c in changes{let record=if c.bytes.is_some(){format!("{} {}\t{}\0",c.mode,c.oid,c.path)}else{format!("0 {}\t{}\0","0".repeat(hash_len),c.path)};data.extend_from_slice(record.as_bytes());}data}
async fn preview(rt:&Runtime,w:&Project,paths:Vec<String>)->Result<Preview>{
    let paths=selected(rt,w,paths).await?;let head=head(rt,w).await?;let temp=tempfile::Builder::new().prefix("git-review-").tempdir_in(rt.config.security.data_dir.join("tmp"))?;let index=temp.path().join("index");let objects=temp.path().join("objects");std::fs::create_dir(&objects)?;let env=Environment{index:Some(index.clone()),objects:Some(objects)};
    let base=if head=="UNBORN"{good(rt,w,args(&["read-tree","--empty"]),&env,None).await?;oid(good(rt,w,args(&["hash-object","-t","tree","-w","--stdin"]),&env,Some(vec![])).await?)?}else{good(rt,w,vec!["read-tree".into(),head.clone()],&env,None).await?;head.clone()};
    let mut changes=Vec::new();let mut total=0;
    for path in &paths{let data=w.root.read_optional(path,rt.config.limits.max_file_bytes)?;let (mode,id)=if let Some(bytes)=&data{total+=bytes.len();if total>16*1024*1024{bail!("Review input exceeds 16 MiB; use smaller commits");}let id=oid(good(rt,w,args(&["hash-object","-w","--no-filters","--stdin"]),&env,Some(bytes.clone())).await?)?;let mode=if w.root.metadata(path)?.mode()&0o111!=0{"100755"}else{"100644"};(mode.to_owned(),id)}else{("0".into(),String::new())};changes.push(Change{path:path.clone(),bytes:data,mode,oid:id});}
    if !changes.is_empty(){good(rt,w,args(&["update-index","-z","--index-info"]),&env,Some(index_records(&changes,base.len()))).await?;}
    let mut command=args(&["diff","--cached","--binary","--full-index","--no-ext-diff","--no-textconv","--ignore-submodules=all","--no-color"]);command.push(base);command.push("--".into());command.extend(paths.clone());
    let diff=String::from_utf8(good(rt,w,command,&env,None).await?).context("Git diff is not UTF-8")?;
    if diff.len()>rt.config.limits.max_output_bytes{bail!("Diff exceeds max_output_bytes; select fewer/smaller files");}
    let review=util::digest(serde_json::to_vec(&(&head,&paths,&diff))?);Ok(Preview{_temp:temp,index,head,paths,diff,review,changes})
}
pub async fn diff(rt:&Runtime,w:&Project,a:DiffArgs)->Result<Value>{preflight(rt,w).await?;let p=preview(rt,w,a.paths).await?;Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"paths":p.paths,"head":p.head,"diff_sha256":p.review,"diff":p.diff,"has_changes":!p.diff.is_empty(),"commit_policy":"Pass these exact paths/head/diff_sha256 to git_commit. Selected pre-staged changes are refused; unrelated staging is preserved. Raw file bytes are used (no clean filters/LFS conversion)."}))}

pub async fn commit(rt:&Runtime,w:&Project,a:CommitArgs)->Result<Value>{
    w.commit_allowed()?;preflight(rt,w).await?;
    if a.paths.is_empty(){bail!("git_commit requires explicit nonempty paths");}
    if a.message.trim().is_empty()||a.message.len()>4096||a.message.contains('\0'){bail!("Invalid commit message");}
    let gitdir=repo(w)?;let lockpath=gitdir.join("index.lock");
    let lockfile=OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&lockpath).context("GIT_BUSY: index.lock exists; never remove another process's lock automatically")?;
    let mut lock=IndexLock{path:lockpath,file:lockfile,keep:false,published:false};
    let branch_raw=good(rt,w,args(&["symbolic-ref","--quiet","HEAD"]),&Environment::default(),None).await.context("Commits require a checked-out branch; detached HEAD is refused")?;let branch=String::from_utf8(branch_raw)?.trim().to_owned();if !branch.starts_with("refs/heads/"){bail!("Unexpected HEAD reference");}
    let p=preview(rt,w,a.paths).await?;
    if p.head!=a.expected_head||p.review!=a.expected_diff_sha256{bail!("GIT_CONFLICT: HEAD or reviewed file contents changed; call git_diff again");}if p.diff.is_empty(){bail!("Nothing to commit");}
    let mut check=args(&["diff","--cached","--name-only","-z","--no-ext-diff","--no-textconv","--ignore-submodules=all","--"]);check.extend(p.paths.clone());
    if !good(rt,w,check,&Environment::default(),None).await?.is_empty(){bail!("STAGED_CONFLICT: a selected path already contains staged user work; unstage/review it yourself before retrying");}
    // Materialize only the reviewed blob bytes; hash-object --no-filters prevents local clean-filter programs.
    for c in &p.changes{if let Some(bytes)=&c.bytes{let id=oid(good(rt,w,args(&["hash-object","-w","--no-filters","--stdin"]),&Environment::default(),Some(bytes.clone())).await?)?;if id!=c.oid{bail!("Git object format changed during commit");}}}
    let temp_env=Environment{index:Some(p.index.clone()),objects:None};
    let tree=oid(good(rt,w,args(&["write-tree"]),&temp_env,None).await?)?;
    let mut create=vec!["commit-tree".into(),tree];if p.head!="UNBORN"{create.push("-p".into());create.push(p.head.clone());}
    let commit_id=oid(good(rt,w,create,&Environment::default(),Some(format!("{}\n",a.message.trim()).into_bytes())).await?)?;
    // Merge just the selected entries into a copy of the *real* index, preserving unrelated staging.
    let merged=p._temp.path().join("merged-index");let index_path=gitdir.join("index");
    if index_path.exists(){let md=std::fs::symlink_metadata(&index_path)?;if !md.is_file()||md.file_type().is_symlink()||md.nlink()!=1||md.len()>64*1024*1024{bail!("Unsafe/oversized Git index");}std::fs::copy(&index_path,&merged)?;}else{let env=Environment{index:Some(merged.clone()),objects:None};good(rt,w,args(&["read-tree","--empty"]),&env,None).await?;}
    let merged_env=Environment{index:Some(merged.clone()),objects:None};good(rt,w,args(&["update-index","-z","--index-info"]),&merged_env,Some(index_records(&p.changes,commit_id.len()))).await?;
    let index_bytes=std::fs::read(&merged)?;lock.file.write_all(&index_bytes)?;lock.file.sync_all()?;
    let current_branch=String::from_utf8(good(rt,w,args(&["symbolic-ref","--quiet","HEAD"]),&Environment::default(),None).await?)?;if current_branch.trim()!=branch{bail!("Branch changed during commit");}
    let journal=rt.config.security.data_dir.join("git-journal").join(format!("{}.json",util::random_secret()?));
    util::private_create(&journal,&serde_json::to_vec_pretty(&json!({"workspace":w.workspace_id,"project":w.config.id,"branch":branch,"old_head":p.head,"new_head":commit_id,"index_lock":lock.path,"index_sha256":util::digest(&index_bytes),"note":"If HEAD=new_head and index.lock has the recorded hash, publish that lock file as index. Otherwise inspect manually; never reset or discard work automatically."}))?)?;
    let expected=if p.head=="UNBORN"{"0".repeat(commit_id.len())}else{p.head.clone()};
    lock.keep=true; // Retain journal/index.lock if this mutation is cancelled or its result is ambiguous.
    let update=good(rt,w,vec!["update-ref".into(),"-m".into(),format!("EndlessVibe: {}",a.message.lines().next().unwrap_or("commit")),branch.clone(),commit_id.clone(),expected],&Environment::default(),None).await;
    if let Err(e)=update{
        let actual=head(rt,w).await;
        if actual.as_ref().is_ok_and(|h|h==&commit_id){/* The ref update committed but its response failed: finish index publication. */}
        else if actual.as_ref().is_ok_and(|h|h==&p.head){lock.keep=false;let _=std::fs::remove_file(&journal);return Err(e).context("Branch compare-and-swap failed; working tree and real index are unchanged");}
        else{bail!("GIT_UPDATE_AMBIGUOUS: index.lock and {} were retained for manual recovery; do not reset the repository: {e}",journal.display());}
    }
    lock.keep=true;
    if let Err(e)=std::fs::rename(&lock.path,&index_path){bail!("COMMIT_PARTIALLY_PUBLISHED: HEAD is now {commit_id}; index.lock and {} were retained for recovery: {e}",journal.display());}
    lock.published=true;let durability_warning=File::open(&gitdir).and_then(|d|d.sync_all()).is_err();let _=std::fs::remove_file(&journal);let operation_diff=p.diff.clone();
    Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"commit":commit_id,"branch":branch,"paths":p.paths,"pushed":false,"unrelated_staging_preserved":true,"working_tree_not_rewritten":true,"durability_warning":durability_warning,"_operation_diff":operation_diff}))
}
#[cfg(test)]mod tests{use super::*;#[test]fn status_handles_renames_and_hides_sensitive_paths(){let v=parse_status(b" M src/main.rs\0R  new.rs\0old.rs\0?? .env\0").unwrap();assert_eq!(v.len(),2);assert_eq!(v[1]["original_path"],"old.rs");}#[test]fn object_ids_are_validated(){assert!(oid(b"bad\n".to_vec()).is_err());assert!(oid(format!("{}\n","a".repeat(40)).into_bytes()).is_ok());}}
