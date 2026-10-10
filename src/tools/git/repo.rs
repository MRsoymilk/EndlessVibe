use crate::{runtime::Runtime,tools::{process,types::LogArgs},util,workspace::Project};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{fs::OpenOptions,io::{Read,Write},path::PathBuf};
#[cfg(unix)] use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)] use std::os::windows::fs::OpenOptionsExt;
use tokio::process::Command;

#[derive(Clone,Default)]pub(super) struct Environment{pub(super) index:Option<PathBuf>,pub(super) objects:Option<PathBuf>}

pub(super) fn repo(w:&Project)->Result<PathBuf>{
    w.root.unchanged_root()?;let dir=w.root.path.join(".git");let md=std::fs::symlink_metadata(&dir).context("Project is not a standalone Git repository; linked worktrees and subdirectory repositories are not supported by this release")?;
    if !md.is_dir()||md.file_type().is_symlink(){bail!(".git must be a real directory (linked worktrees/bare repositories are not supported)");}
    #[cfg(unix)]
    if dir.to_string_lossy().contains([':', '\n', '\r']){bail!("Git repository paths containing ':' or newlines are not supported");}
    #[cfg(windows)]
    if dir.to_string_lossy().chars().skip(2).any(|c|matches!(c,':'|'\n'|'\r')){bail!("Git repository paths containing alternate data streams or newlines are not supported");}
    Ok(dir)
}

pub(super) async fn preflight(rt:&Runtime,w:&Project)->Result<()>{
    let dir=repo(w)?;
    for part in ["config","HEAD","index","objects","refs","packed-refs","shallow"]{let path=dir.join(part);if let Ok(md)=std::fs::symlink_metadata(&path){if md.file_type().is_symlink(){bail!("Git metadata symlinks are not supported: {part}");}}}
    for part in ["commondir","objects/info/alternates","objects/info/http-alternates"]{if dir.join(part).exists(){bail!("External/shared Git metadata is not supported: {part}");}}
    let path=dir.join("config");let mut data=Vec::new();
    if path.exists(){let mut options=OpenOptions::new();options.read(true);
        #[cfg(unix)] options.custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK);
        #[cfg(windows)] options.custom_flags(0x0020_0000);
        let file=options.open(&path)?;let md=file.metadata()?;
        if !md.is_file()||!crate::platform::single_link(&md)||md.len()>262144||std::fs::symlink_metadata(&path)?.file_type().is_symlink(){bail!("Unsafe/oversized Git config");}
        file.take(262145).read_to_end(&mut data)?;if data.len()>262144{bail!("Oversized Git config");}}
    let mut temp=tempfile::Builder::new().prefix("git-config-").tempfile_in(rt.config.security.data_dir.join("tmp"))?;temp.write_all(&data)?;temp.flush()?;
    let mut cmd=Command::new(&rt.config.git.executable);process::clean_environment(&mut cmd,&rt.config.execution.path);cmd.current_dir(rt.config.security.data_dir.join("empty-home")).env("HOME",rt.config.security.data_dir.join("empty-home")).env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL",crate::platform::null_device()).args(["config","--null","--no-includes","--file"]).arg(temp.path()).arg("--list");
    let result=process::capture(cmd,None,524288,5).await?;if result.code!=Some(0){bail!("Git config cannot be safely parsed");}
    for line in result.stdout.split(|b|*b==0).filter(|b|!b.is_empty()){let key=String::from_utf8_lossy(line.split(|b|*b==b'\n').next().unwrap_or_default()).to_ascii_lowercase();if ["include.","includeif.","filter.","diff.","merge.","alias.","credential.","gpg.","lfs."].iter().any(|p|key.starts_with(*p))||(key.starts_with("remote.")&&key.ends_with(".promisor"))||(key.starts_with("extensions.")&&key!="extensions.objectformat")||matches!(key.as_str(),"core.fsmonitor"|"core.sparsecheckout"|"core.sparsecheckoutcone"){bail!("Repository config key '{key}' needs manual review; helper/filter/include/sparse/partial-clone configurations are not supported by the dedicated Git tools. Existing config was not changed.");}}
    Ok(())
}

pub(super) fn command(rt:&Runtime,w:&Project,env:&Environment)->Result<Command>{
    let gitdir=repo(w)?;let mut c=Command::new(&rt.config.git.executable);process::clean_environment(&mut c,&rt.config.execution.path);
    c.current_dir(&w.root.path).env("HOME",rt.config.security.data_dir.join("empty-home")).env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL",crate::platform::null_device()).env("GIT_ATTR_NOSYSTEM","1").env("GIT_LITERAL_PATHSPECS","1").env("GIT_TERMINAL_PROMPT","0").env("GIT_OPTIONAL_LOCKS","0").env("GIT_NO_REPLACE_OBJECTS","1").env("GIT_NO_LAZY_FETCH","1").env("GIT_AUTHOR_NAME",&rt.config.git.author_name).env("GIT_AUTHOR_EMAIL",&rt.config.git.author_email).env("GIT_COMMITTER_NAME",&rt.config.git.author_name).env("GIT_COMMITTER_EMAIL",&rt.config.git.author_email);
    c.arg("--no-pager").arg("--git-dir").arg(&gitdir).arg("--work-tree").arg(&w.root.path);
    let null=crate::platform::null_device();
    let hooks=format!("core.hooksPath={null}");let attrs=format!("core.attributesFile={null}");
    let deny=if cfg!(windows){"cmd /c exit 1"}else{"/bin/false"};
    let git_proxy=format!("core.gitProxy={deny}");let ssh_command=format!("core.sshCommand={deny}");
    for setting in [hooks.as_str(),"core.fsmonitor=false","commit.gpgSign=false","tag.gpgSign=false",attrs.as_str(),"diff.external=","core.quotePath=false","submodule.recurse=false","protocol.allow=never",git_proxy.as_str(),ssh_command.as_str()]{c.arg("-c").arg(setting);}
    if let Some(index)=&env.index{c.env("GIT_INDEX_FILE",index);}if let Some(objects)=&env.objects{c.env("GIT_OBJECT_DIRECTORY",objects).env("GIT_ALTERNATE_OBJECT_DIRECTORIES",gitdir.join("objects"));}Ok(c)
}

pub(super) async fn run(rt:&Runtime,w:&Project,args:Vec<String>,env:&Environment,input:Option<Vec<u8>>)->Result<process::Captured>{let mut cmd=command(rt,w,env)?;cmd.args(args);process::capture(cmd,input,rt.config.limits.max_output_bytes.max(4*1024*1024),20).await}
pub(super) fn args(values:&[&str])->Vec<String>{values.iter().map(|s|(*s).to_owned()).collect()}
pub(super) async fn good(rt:&Runtime,w:&Project,values:Vec<String>,env:&Environment,input:Option<Vec<u8>>)->Result<Vec<u8>>{let out=run(rt,w,values,env,input).await?;if out.code!=Some(0){bail!("Git failed: {}",util::bounded_text(&String::from_utf8_lossy(&out.stderr),2048));}Ok(out.stdout)}
pub(super) async fn head(rt:&Runtime,w:&Project)->Result<String>{let out=run(rt,w,args(&["rev-parse","--verify","--quiet","HEAD"]),&Environment::default(),None).await?;match out.code{Some(0)=>Ok(String::from_utf8(out.stdout)?.trim().to_owned()),Some(1)=>Ok("UNBORN".into()),_=>bail!("Cannot determine repository HEAD")}}
pub(super) async fn branch(rt:&Runtime,w:&Project)->Result<Option<String>>{let out=run(rt,w,args(&["symbolic-ref","--quiet","--short","HEAD"]),&Environment::default(),None).await?;match out.code{Some(0)=>Ok(Some(String::from_utf8(out.stdout)?.trim().to_owned())),Some(1)=>Ok(None),_=>bail!("Cannot determine repository branch")}}
pub(super) fn oid(bytes:Vec<u8>)->Result<String>{let value=String::from_utf8(bytes)?.trim().to_owned();if !matches!(value.len(),40|64)||!value.bytes().all(|c|c.is_ascii_hexdigit()){bail!("Git returned an invalid object ID");}Ok(value)}

fn parse_status(bytes:&[u8])->Result<Vec<Value>>{let mut values=bytes.split(|b|*b==0).filter(|b|!b.is_empty());let mut out=Vec::new();while let Some(value)=values.next(){if value.len()<4{bail!("Invalid Git status record");}let x=value[0] as char;let y=value[1] as char;let path=String::from_utf8(value[3..].to_vec())?;let original=if matches!(x,'R'|'C')||matches!(y,'R'|'C'){values.next().map(|v|String::from_utf8(v.to_vec())).transpose()?}else{None};if crate::security::paths::relative(&path,false).is_ok(){out.push(json!({"index":x.to_string(),"worktree":y.to_string(),"path":path,"original_path":original.filter(|p|crate::security::paths::relative(p,false).is_ok())}));}}Ok(out)}

pub async fn status(rt:&Runtime,w:&Project)->Result<Value>{preflight(rt,w).await?;let raw=good(rt,w,args(&["status","--porcelain=v1","-z","--untracked-files=all","--ignore-submodules=all"]),&Environment::default(),None).await?;let mut entries=parse_status(&raw)?;let total_entries=entries.len();let truncated=total_entries>200;entries.truncate(200);Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"branch":branch(rt,w).await?,"head":head(rt,w).await?,"entries":entries,"total_entries":total_entries,"truncated":truncated,"sensitive_paths_omitted":true}))}
pub async fn log(rt:&Runtime,w:&Project,a:LogArgs)->Result<Value>{preflight(rt,w).await?;if a.limit==0||a.limit>100{bail!("limit must be 1..100");}if head(rt,w).await?=="UNBORN"{return Ok(json!({"commits":[]}));}let values=vec!["log".into(),format!("-{}",a.limit),"--format=%H%x00%an%x00%aI%x00%s%x00".into(),"--no-decorate".into()];let raw=good(rt,w,values,&Environment::default(),None).await?;let mut out=Vec::new();let fields=raw.split(|b|*b==0).collect::<Vec<_>>();for chunk in fields.chunks(4){if chunk.len()==4{out.push(json!({"commit":String::from_utf8_lossy(chunk[0]).trim(),"author":String::from_utf8_lossy(chunk[1]),"date":String::from_utf8_lossy(chunk[2]),"subject":String::from_utf8_lossy(chunk[3])}));}}Ok(json!({"commits":out}))}

#[cfg(test)]mod tests{use super::*;#[test]fn status_handles_renames_and_hides_sensitive_paths(){let v=parse_status(b" M src/main.rs\0R  new.rs\0old.rs\0?? .env\0").unwrap();assert_eq!(v.len(),2);assert_eq!(v[1]["original_path"],"old.rs");}#[test]fn object_ids_are_validated(){assert!(oid(b"bad\n".to_vec()).is_err());assert!(oid(format!("{}\n","a".repeat(40)).into_bytes()).is_ok());}}
