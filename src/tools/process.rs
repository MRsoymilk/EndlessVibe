use crate::{config::Config, workspace::Project};
use anyhow::{bail, Context, Result};
use std::{path::{Path,PathBuf}, process::Stdio, time::Duration};
use tokio::{io::{AsyncRead,AsyncReadExt,AsyncWriteExt}, process::Command};

pub fn kill_group(pid:u32,signal:i32){if pid>1{unsafe{libc::kill(-(pid as i32),signal);}}}
pub struct GroupGuard{pid:Option<u32>}
impl GroupGuard{pub fn new(pid:u32)->Self{Self{pid:Some(pid)}}pub fn kill(&mut self){if let Some(pid)=self.pid.take(){kill_group(pid,libc::SIGKILL);}}}
impl Drop for GroupGuard{fn drop(&mut self){self.kill();}}
pub async fn terminate_group(pid:u32){kill_group(pid,libc::SIGTERM);tokio::time::sleep(Duration::from_millis(200)).await;kill_group(pid,libc::SIGKILL);}
async fn drain<R:AsyncRead+Unpin>(mut r:R,limit:usize)->Result<(Vec<u8>,bool)>{let mut bytes=Vec::new();let mut buf=[0u8;8192];let mut truncated=false;loop{let n=r.read(&mut buf).await?;if n==0{break;}let take=n.min(limit.saturating_sub(bytes.len()));bytes.extend_from_slice(&buf[..take]);truncated|=take<n;}Ok((bytes,truncated))}
pub struct Captured{pub code:Option<i32>,pub stdout:Vec<u8>,pub stderr:Vec<u8>}
pub async fn capture(mut cmd:Command,input:Option<Vec<u8>>,limit:usize,seconds:u64)->Result<Captured>{
    cmd.stdin(if input.is_some(){Stdio::piped()}else{Stdio::null()}).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).process_group(0);
    let mut child=cmd.spawn().context("Cannot start configured executable")?;let pid=child.id().context("Missing process ID")?;let mut group=GroupGuard::new(pid);
    let mut out=tokio::spawn(drain(child.stdout.take().context("stdout missing")?,limit));let mut err=tokio::spawn(drain(child.stderr.take().context("stderr missing")?,limit));
    let writer=if let Some(input)=input{let mut stdin=child.stdin.take().context("stdin missing")?;Some(tokio::spawn(async move{stdin.write_all(&input).await?;stdin.shutdown().await}))}else{None};
    let status=match tokio::time::timeout(Duration::from_secs(seconds),child.wait()).await{Ok(result)=>result?,Err(_)=>{terminate_group(pid).await;let _=child.wait().await;out.abort();err.abort();if let Some(w)=writer{w.abort();}bail!("Command timed out after {seconds}s");}};
    group.kill();
    let output=tokio::time::timeout(Duration::from_secs(2),&mut out).await;
    let error=tokio::time::timeout(Duration::from_secs(2),&mut err).await;
    if let Some(w)=writer{if !w.is_finished(){w.abort();}else{let _=w.await;}}
    if output.is_err()||error.is_err(){out.abort();err.abort();bail!("Command descendant kept output pipes open");}
    let (stdout,ot)=output???;let (stderr,et)=error???;
    if ot||et{bail!("Command output exceeds limit; narrow the query");}
    Ok(Captured{code:status.code(),stdout,stderr})
}

pub fn clean_environment(cmd:&mut Command,path:&str){cmd.env_clear().env("PATH",path).env("LANG","C.UTF-8").env("LC_ALL","C.UTF-8").env("TERM","dumb");}
fn validated_executable(path:&str,program:&str)->Result<PathBuf>{
    if program.is_empty()||program.len()>64||!program.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b)){bail!("program must be a simple executable name; use args for arguments");}
    for dir in path.split(':'){let p=Path::new(dir).join(program);if p.is_file(){return Ok(p);}}
    bail!("Executable {program} not found in configured PATH")
}

fn pre_exec_nproc_limit(backend:&str,max_processes:u64)->Option<u64>{
    // RLIMIT_NPROC is counted against every thread/process owned by the real UID.
    // Applying it to bwrap before namespace creation can make bwrap's clone() fail
    // with EAGAIN when the desktop user already owns >= max_processes tasks.
    // Host execution can still opt into this coarse per-UID limit; bubblewrap must
    // finish creating its namespaces before any process-count isolation is applied.
    (backend=="host").then_some(max_processes)
}

fn bubblewrap_base_command(config:&Config)->Result<Command>{
    if !config.execution.bubblewrap.is_file(){bail!("bubblewrap is missing; install sys-apps/bubblewrap on Gentoo, or explicitly opt into unsafe host execution");}
    let mut c=Command::new(&config.execution.bubblewrap);clean_environment(&mut c,"/usr/bin:/bin");
    c.args(["--die-with-parent","--new-session","--unshare-all","--clearenv"]);
    if config.execution.allow_network{c.arg("--share-net");}
    for p in ["/usr","/bin","/sbin","/lib","/lib64"]{let p=Path::new(p);if p.exists(){c.arg("--ro-bind").arg(p).arg(p);}}
    c.args(["--proc","/proc","--dev","/dev","--tmpfs","/tmp","--dir","/tmp/home","--dir","/etc"]);
    for p in ["/etc/ld.so.cache","/etc/ld.so.conf","/etc/ld.so.conf.d","/etc/localtime"]{if Path::new(p).exists(){c.arg("--ro-bind").arg(p).arg(p);}}
    if config.execution.allow_network{for p in ["/etc/resolv.conf","/etc/hosts","/etc/ssl/certs"]{if Path::new(p).exists(){c.arg("--ro-bind").arg(p).arg(p);}}}
    for mount in &config.execution.readonly_mounts{c.arg("--ro-bind").arg(&mount.source).arg(&mount.target);}
    Ok(c)
}

pub async fn probe_bubblewrap(config:&Config)->Result<Vec<(String,String)>>{
    if config.execution.backend!="bubblewrap"{bail!("Configured execution backend is not bubblewrap");}
    let mut namespace=bubblewrap_base_command(config)?;
    namespace.args(["--setenv","HOME","/tmp/home","--setenv","PATH",&config.execution.path,"--setenv","LANG","C.UTF-8","--setenv","LC_ALL","C.UTF-8","--setenv","TERM","dumb","--","/bin/sh","-c","exit 0"]);
    let result=capture(namespace,None,16384,5).await?;
    if result.code!=Some(0){bail!("bubblewrap sandbox probe failed: {}",crate::util::bounded_text(&String::from_utf8_lossy(&result.stderr),2048));}
    let mut probe=bubblewrap_base_command(config)?;
    probe.args(["--setenv","HOME","/tmp/home","--setenv","PATH",&config.execution.path,"--setenv","LANG","C.UTF-8","--setenv","LC_ALL","C.UTF-8","--setenv","TERM","dumb","--","/bin/sh","-c","missing=0; for p do if v=$(command -v \"$p\" 2>/dev/null); then printf 'OK\\t%s\\t%s\\n' \"$p\" \"$v\"; else printf 'MISSING\\t%s\\n' \"$p\"; missing=1; fi; done; exit $missing","probe"]);probe.args(&config.execution.allowed_programs);
    let result=capture(probe,None,262144,10).await?;let stdout=String::from_utf8(result.stdout).context("Sandbox toolchain probe output is not UTF-8")?;let mut visible=Vec::new();let mut missing=Vec::new();for line in stdout.lines(){let mut fields=line.split('\t');match fields.next(){Some("OK")=>{if let (Some(program),Some(path))=(fields.next(),fields.next()){visible.push((program.to_owned(),path.to_owned()));}},Some("MISSING")=>{if let Some(program)=fields.next(){missing.push(program.to_owned());}},_=>{}}}
    if !missing.is_empty(){bail!("bubblewrap sandbox probe: configured program(s) not visible: {}. execution.path={}. If Cargo/Rustup lives under ~/.rustup, mount only the active toolchain directory read-only to /opt/rust and prepend /opt/rust/bin to execution.path; do not mount HOME or credential directories",missing.join(", "),config.execution.path);}
    if result.code!=Some(0){bail!("bubblewrap program visibility probe failed: {}",crate::util::bounded_text(&String::from_utf8_lossy(&result.stderr),2048));}
    Ok(visible)
}

fn git_network_subcommand(args:&[String])->bool{args.iter().any(|arg|matches!(arg.as_str(),"push"|"fetch"|"pull"|"clone"|"ls-remote"|"remote"|"submodule"))}
pub fn build_job_command(config:&Config,w:&Project,program:&str,args:&[String],cwd:&str,shell:bool)->Result<Command>{
    w.exec_allowed()?;
    if program=="git"&&git_network_subcommand(args){bail!("Network Git subcommands are disabled in run_command; allow_git_mutation only permits local repository mutations");}
    if args.len()>128||args.iter().any(|a|a.contains('\0')||a.len()>65536)||args.iter().map(|a|a.len()).sum::<usize>()>131072{bail!("Command arguments exceed limits");}
    if shell&&!config.execution.allow_shell{bail!("run_shell is disabled; set execution.allow_shell=true locally after reviewing the risks");}
    if !shell&&!config.execution.allowed_programs.iter().any(|p|p==program){bail!("Program is not in execution.allowed_programs");}
    let host_cwd=w.root.directory_path(cwd)?;
    let mut command=match config.execution.backend.as_str(){
        "disabled"=>bail!("Command execution backend is disabled"),
        "host"=>{
            if !config.execution.acknowledge_unsafe_host_execution{bail!("Host execution has not been explicitly authorized");}
            let executable=if shell{PathBuf::from("/bin/bash")}else{validated_executable(&config.execution.path,program)?};
            let mut c=Command::new(executable);clean_environment(&mut c,&config.execution.path);c.current_dir(host_cwd);
            // Explicit host mode is not a sandbox; no service token/key is inherited.
            if let Some(home)=std::env::var_os("HOME"){c.env("HOME",home);}c.args(args);c
        }
        "bubblewrap"=>{
            let cache=config.security.data_dir.join("exec-cache").join(&w.workspace_id).join(&w.config.id);crate::util::private_dir(&cache)?;
            let mut c=bubblewrap_base_command(config)?;
            c.arg("--bind").arg(&w.root.path).arg("/workspace").arg("--bind").arg(&cache).arg("/cache");
            // Git metadata is read-only by default. Explicit allow_git_mutation keeps the project's
            // own .git writable while HOME, credentials, service state and network remain isolated.
            let git_metadata=w.root.path.join(".git");if git_metadata.exists(){if std::fs::symlink_metadata(&git_metadata)?.file_type().is_symlink(){bail!("Sandbox refuses symlinked Git metadata");}if !w.config.allow_git_mutation{c.arg("--ro-bind").arg(&git_metadata).arg("/workspace/.git");}}
            let inside=if cwd=="."{PathBuf::from("/workspace")}else{Path::new("/workspace").join(cwd)};
            c.arg("--chdir").arg(inside).args(["--setenv","HOME","/tmp/home","--setenv","PATH",&config.execution.path,"--setenv","CARGO_HOME","/cache/cargo","--setenv","XDG_CACHE_HOME","/cache/xdg","--setenv","LANG","C.UTF-8","--setenv","LC_ALL","C.UTF-8","--setenv","TERM","dumb"]);
            c.arg("--").arg(if shell{"/bin/bash"}else{program}).args(args);c
        }
        _=>bail!("Unknown execution backend"),
    };
    if !shell&&program=="git"{command.env("GIT_AUTHOR_NAME",&config.git.author_name).env("GIT_AUTHOR_EMAIL",&config.git.author_email).env("GIT_COMMITTER_NAME",&config.git.author_name).env("GIT_COMMITTER_EMAIL",&config.git.author_email).env("GIT_MERGE_AUTOEDIT","no").env("GIT_EDITOR","true");}
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).process_group(0);
    let address_space=config.execution.memory_limit_mb.saturating_mul(1024*1024);let processes=pre_exec_nproc_limit(&config.execution.backend,config.execution.max_processes);let cpu=config.limits.command_timeout_seconds+5;
    // SAFETY: only async-signal-safe Linux syscalls are used in this pre_exec closure.
    unsafe{command.pre_exec(move||{
        for (resource,value) in [(libc::RLIMIT_AS,address_space),(libc::RLIMIT_CPU,cpu)]{let lim=libc::rlimit{rlim_cur:value as libc::rlim_t,rlim_max:value as libc::rlim_t};if libc::setrlimit(resource,&lim)!=0{return Err(std::io::Error::last_os_error());}}
        if let Some(value)=processes{let lim=libc::rlimit{rlim_cur:value as libc::rlim_t,rlim_max:value as libc::rlim_t};if libc::setrlimit(libc::RLIMIT_NPROC,&lim)!=0{return Err(std::io::Error::last_os_error());}}
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS,1 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong)!=0{return Err(std::io::Error::last_os_error());}Ok(())
    });}
    Ok(command)
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn bubblewrap_does_not_limit_launcher_nproc(){assert_eq!(pre_exec_nproc_limit("bubblewrap",256),None);}
    #[test]fn host_keeps_configured_nproc_limit(){assert_eq!(pre_exec_nproc_limit("host",256),Some(256));}
    #[test]fn network_git_subcommands_are_blocked(){for sub in ["push","fetch","pull","clone","ls-remote","remote","submodule"]{assert!(git_network_subcommand(&[sub.into()]));}for sub in ["status","switch","merge","branch","add","commit","rebase"]{assert!(!git_network_subcommand(&[sub.into()]));}}
}
