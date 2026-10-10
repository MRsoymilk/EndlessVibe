use crate::{config::Config, workspace::Project};
use anyhow::{bail, Context, Result};
use serde_json::{json,Value};
use std::{collections::BTreeMap,path::{Path,PathBuf}, process::Stdio, time::Duration};
use tokio::{io::{AsyncRead,AsyncReadExt,AsyncWriteExt}, process::Command};

#[cfg(unix)]
pub fn kill_group(pid:u32,signal:i32){if pid>1{unsafe{libc::kill(-(pid as i32),signal);}}}
pub struct GroupGuard{pid:Option<u32>}
impl GroupGuard{pub fn new(pid:u32)->Self{Self{pid:Some(pid)}}pub fn kill(&mut self){if let Some(pid)=self.pid.take(){#[cfg(unix)]kill_group(pid,libc::SIGKILL);#[cfg(windows)]let _=pid;}}}
impl Drop for GroupGuard{fn drop(&mut self){self.kill();}}
#[cfg(unix)]
pub async fn terminate_group(pid:u32){kill_group(pid,libc::SIGTERM);tokio::time::sleep(Duration::from_millis(200)).await;kill_group(pid,libc::SIGKILL);}
#[cfg(windows)]
pub async fn terminate_group(pid:u32){
    // Windows keeps the child handle alive while waiting, preventing PID reuse.
    // taskkill /T terminates its process tree without borrowing Child::wait.
    let binary=std::env::var_os("SystemRoot").map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(r"C:\Windows")).join("System32").join("taskkill.exe");
    let mut cmd=Command::new(binary);
    cmd.args(["/F","/T","/PID",&pid.to_string()]);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    let _=tokio::time::timeout(Duration::from_secs(5),cmd.status()).await;
}
async fn drain<R:AsyncRead+Unpin>(mut r:R,limit:usize)->Result<(Vec<u8>,bool)>{let mut bytes=Vec::new();let mut buf=[0u8;8192];let mut truncated=false;loop{let n=r.read(&mut buf).await?;if n==0{break;}let take=n.min(limit.saturating_sub(bytes.len()));bytes.extend_from_slice(&buf[..take]);truncated|=take<n;}Ok((bytes,truncated))}
pub struct Captured{pub code:Option<i32>,pub stdout:Vec<u8>,pub stderr:Vec<u8>}
pub async fn capture(mut cmd:Command,input:Option<Vec<u8>>,limit:usize,seconds:u64)->Result<Captured>{
    cmd.stdin(if input.is_some(){Stdio::piped()}else{Stdio::null()}).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]cmd.process_group(0);
    let mut child=cmd.spawn().context("Cannot start configured executable")?;let pid=child.id().context("Missing process ID")?;let mut group=GroupGuard::new(pid);
    let mut out=tokio::spawn(drain(child.stdout.take().context("stdout missing")?,limit));let mut err=tokio::spawn(drain(child.stderr.take().context("stderr missing")?,limit));
    let writer=if let Some(input)=input{let mut stdin=child.stdin.take().context("stdin missing")?;Some(tokio::spawn(async move{stdin.write_all(&input).await?;stdin.shutdown().await}))}else{None};
    let status=match tokio::time::timeout(Duration::from_secs(seconds),child.wait()).await{Ok(result)=>result?,Err(_)=>{terminate_group(pid).await;#[cfg(windows)]let _=child.kill().await;let _=child.wait().await;out.abort();err.abort();if let Some(w)=writer{w.abort();}bail!("Command timed out after {seconds}s");}};
    group.kill();
    let output=tokio::time::timeout(Duration::from_secs(2),&mut out).await;
    let error=tokio::time::timeout(Duration::from_secs(2),&mut err).await;
    if let Some(w)=writer{if !w.is_finished(){w.abort();}else{let _=w.await;}}
    if output.is_err()||error.is_err(){out.abort();err.abort();bail!("Command descendant kept output pipes open");}
    let (stdout,ot)=output???;let (stderr,et)=error???;
    if ot||et{bail!("Command output exceeds limit; narrow the query");}
    Ok(Captured{code:status.code(),stdout,stderr})
}

pub fn clean_environment(cmd:&mut Command,path:&str){
    cmd.env_clear().env("PATH",path).env("LANG","C.UTF-8").env("LC_ALL","C.UTF-8").env("TERM","dumb");
    #[cfg(windows)]for key in ["SystemRoot","WINDIR","TEMP","TMP","USERPROFILE"]{
        if let Some(value)=std::env::var_os(key){cmd.env(key,value);}
    }
}
#[derive(Clone,Debug)]pub struct JobSummaryContract{pub host_path:PathBuf,pub exposed_path:String}
pub fn job_summary_host_path(config:&Config,workspace:&str,project:&str,job_id:&str)->Result<PathBuf>{if job_id.is_empty()||job_id.len()>128||!job_id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_".contains(&b)){bail!("Invalid job ID for summary contract");}let cache=config.security.data_dir.join("exec-cache").join(workspace).join(project);crate::util::private_dir(&cache)?;Ok(cache.join(format!("job-summary-{job_id}.json")))}
pub fn job_summary_contract(config:&Config,workspace:&str,project:&str,job_id:&str)->Result<JobSummaryContract>{
    let file=format!("job-summary-{job_id}.json");let host_path=job_summary_host_path(config,workspace,project,job_id)?;if host_path.exists(){bail!("Job summary path already exists; refusing to overwrite");}
    let exposed_path=match config.execution.backend.as_str(){"bubblewrap"=>format!("/cache/{file}"),"host"=>host_path.to_string_lossy().into_owned(),"disabled"=>bail!("execution backend is disabled"),_=>bail!("Unknown execution backend")};Ok(JobSummaryContract{host_path,exposed_path})
}
fn simple_program(program:&str)->bool{!program.is_empty()&&program.len()<=64&&program.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b))}
fn validate_job_environment(environment:&BTreeMap<String,String>)->Result<()>{if environment.len()>64{bail!("environment accepts at most 64 variables");}let reserved=["HOME","PATH","CARGO_HOME","XDG_CACHE_HOME","LANG","LC_ALL","TERM","ENDLESSVIBE_JOB_SUMMARY"];let mut total=0usize;for(name,value)in environment{if !crate::config::valid_environment_name(name)||name.len()>128{bail!("environment variable names must match [A-Za-z_][A-Za-z0-9_]* and be at most 128 bytes");}if reserved.contains(&name.as_str()){bail!("environment variable {name} is reserved by EndlessVibe");}if value.contains('\0')||value.len()>65536{bail!("environment variable {name} contains NUL or exceeds 65536 bytes");}total=total.saturating_add(name.len()+value.len());}if total>131072{bail!("environment exceeds 131072 bytes");}Ok(())}
fn validate_job_network(config:&Config,development:bool,requested:bool)->Result<()>{if requested&&config.execution.backend=="bubblewrap"&&!development&&!config.execution.allow_network{bail!("network access is disabled for isolated projects; use execution_profile=development for a trusted development project or enable global execution.allow_network");}Ok(())}
pub fn effective_project_execution(config:&Config,project:&Project,job_environment:&BTreeMap<String,String>,job_network:Option<bool>)->Result<(BTreeMap<String,String>,bool,String,String)>{let mut environment=project.config.environment_map()?;for(name,value)in job_environment{environment.insert(name.clone(),value.clone());}validate_job_environment(&environment)?;let profile=project.config.execution_profile.clone();let network=job_network.unwrap_or_else(||project.config.development());validate_job_network(config,project.config.development(),network)?;let source=if job_network.is_some(){"job_override"}else if project.config.development(){"development_profile"}else{"isolated_profile"}.to_owned();Ok((environment,network,profile,source))}
#[cfg(unix)]
fn existing_unix_socket(path:&Path)->Option<PathBuf>{use std::os::unix::fs::FileTypeExt;let canonical=std::fs::canonicalize(path).ok()?;std::fs::metadata(&canonical).ok()?.file_type().is_socket().then_some(canonical)}
#[cfg(windows)]
fn existing_unix_socket(_path:&Path)->Option<PathBuf>{None}
fn discover_docker_socket()->Option<PathBuf>{if let Ok(host)=std::env::var("DOCKER_HOST"){if let Some(path)=host.strip_prefix("unix://"){if let Some(socket)=existing_unix_socket(Path::new(path)){return Some(socket);}}}for path in [Path::new("/var/run/docker.sock"),Path::new("/run/docker.sock")]{if let Some(socket)=existing_unix_socket(path){return Some(socket);}}if let Some(runtime)=std::env::var_os("XDG_RUNTIME_DIR"){if let Some(socket)=existing_unix_socket(&PathBuf::from(runtime).join("docker.sock")){return Some(socket);}}None}
fn add_programs(out:&mut Vec<String>,items:&[&str]){for item in items{if !out.iter().any(|p|p==item){out.push((*item).to_owned());}}}
fn read_small_manifest(path:&Path)->Option<String>{let meta=std::fs::metadata(path).ok()?;if meta.len()>512*1024{return None;}std::fs::read_to_string(path).ok()}
fn inspect_project_dir(path:&Path,depth:usize,out:&mut Vec<String>){
    if depth>3{return;}let Ok(entries)=std::fs::read_dir(path) else{return};
    for entry in entries.filter_map(|e|e.ok()){
        let p=entry.path();let Ok(ft)=entry.file_type() else{continue};if ft.is_symlink(){continue;}let name=entry.file_name();let name=name.to_string_lossy();
        if ft.is_dir(){if !matches!(name.as_ref(),".git"|"target"|"node_modules"|".venv"|"venv"|"build"|"dist"){inspect_project_dir(&p,depth+1,out);}continue;}
        match name.as_ref(){
            "Cargo.toml"=>{add_programs(out,&["cargo","rustc"]);if let Some(text)=read_small_manifest(&p){if ["sqlx","tokio-postgres","postgres =","diesel"].iter().any(|needle|text.contains(needle)){add_programs(out,&["initdb","postgres","pg_isready","createdb"]);}}},
            "CMakeLists.txt"=>add_programs(out,&["cmake","ninja","make","ctest"]),
            "pyproject.toml"|"requirements.txt"=>{add_programs(out,&["python3"]);if let Some(text)=read_small_manifest(&p){if ["psycopg","asyncpg","postgres"].iter().any(|needle|text.contains(needle)){add_programs(out,&["initdb","postgres","pg_isready","createdb"]);}}},
            "package.json"=>add_programs(out,&["node","npm"]),
            "project.godot"=>add_programs(out,&["godot"]),
            "go.mod"=>add_programs(out,&["go"]),
            "docker-compose.yml"|"docker-compose.yaml"|"compose.yml"|"compose.yaml"=>{if let Some(text)=read_small_manifest(&p){if text.contains("postgres"){add_programs(out,&["initdb","postgres","pg_isready","createdb"]);}}},
            _=>{}
        }
    }
}
pub fn detect_project_programs(project:&Project)->Vec<String>{let mut out=Vec::new();inspect_project_dir(&project.root.path,0,&mut out);out.sort();out.dedup();out}
fn scan_program_mentions(text:&str,out:&mut Vec<String>){
    const AUTO_PROGRAMS:[&str;11]=["cargo","rustc","cmake","ninja","make","ctest","python3","node","npm","go","godot"];
    let mut optional=Vec::<String>::new();let mut required=Vec::<String>::new();
    for line in text.lines(){let line=line.trim();if let Some(rest)=line.strip_prefix("# endlessvibe-optional-programs:"){for program in rest.split_whitespace(){if simple_program(program)&&!optional.iter().any(|p|p==program){optional.push(program.to_owned());}}}if let Some(rest)=line.strip_prefix("# endlessvibe-required-programs:"){for program in rest.split_whitespace(){if simple_program(program)&&!required.iter().any(|p|p==program){required.push(program.to_owned());}}}}
    for program in AUTO_PROGRAMS{let double=format!("\"{program}\"");let single=format!("'{program}'");if (text.contains(&double)||text.contains(&single))&&!optional.iter().any(|p|p==program){add_programs(out,&[program]);}}
    for program in required{if !out.iter().any(|p|p==&program){out.push(program);}}
    if out.iter().any(|p|p=="cargo"){add_programs(out,&["rustc"]);}
}
fn python_compile_only(args:&[String])->bool{args.windows(2).any(|pair|pair[0]=="-m"&&matches!(pair[1].as_str(),"py_compile"|"compileall"))}
pub fn detect_command_programs(project:&Project,cwd:&str,program:&str,args:&[String])->Vec<String>{
    let mut out=Vec::new();
    if program=="python3"&&!python_compile_only(args){
        if let Some(script)=args.iter().find(|arg|!arg.starts_with('-')&&arg.ends_with(".py")){let path=if cwd=="."{PathBuf::from(script)}else{Path::new(cwd).join(script)};if let Some(path)=path.to_str(){if let Ok(bytes)=project.root.read(path,512*1024){if let Ok(text)=std::str::from_utf8(&bytes){scan_program_mentions(text,&mut out);}}}}
    }else if program=="bash"{
        if let Some(pos)=args.iter().position(|arg|arg=="-c"){if let Some(script)=args.get(pos+1){if script.len()<=65536{scan_program_mentions(script,&mut out);}}}
    }
    out.sort();out.dedup();out
}
fn configured_executable(path:&str,program:&str)->Option<PathBuf>{
    for dir in std::env::split_paths(path){let p=dir.join(program);
        if p.is_file(){return Some(p);}
        #[cfg(windows)]{let exe=p.with_extension("exe");if exe.is_file(){return Some(exe);}}
    }None
}
fn validated_executable(path:&str,program:&str)->Result<PathBuf>{
    if !simple_program(program){bail!("program must be a simple executable name; use args for arguments");}
    configured_executable(path,program).with_context(||format!("Executable {program} not found in configured PATH"))
}

fn pre_exec_nproc_limit(backend:&str,max_processes:u64)->Option<u64>{
    // RLIMIT_NPROC is counted against every thread/process owned by the real UID.
    // Applying it to bwrap before namespace creation can make bwrap's clone() fail
    // with EAGAIN when the desktop user already owns >= max_processes tasks.
    // Host execution can still opt into this coarse per-UID limit; bubblewrap must
    // finish creating its namespaces before any process-count isolation is applied.
    (backend=="host").then_some(max_processes)
}

const DIAGNOSTIC_PROGRAMS:[&str;6]=["cargo","rustc","initdb","postgres","pg_isready","createdb"];
fn configured_probe_programs(config:&Config)->Vec<String>{let mut programs=config.execution.allowed_programs.clone();programs.extend(config.execution.required_programs.iter().cloned());programs.sort();programs.dedup();programs}
fn diagnostic_programs(config:&Config)->Vec<String>{let mut programs=configured_probe_programs(config);programs.extend(DIAGNOSTIC_PROGRAMS.map(str::to_owned));programs.sort();programs.dedup();programs}
fn host_executable(program:&str)->Option<String>{let path=std::env::var_os("PATH")?;for dir in std::env::split_paths(&path){let candidate=dir.join(program);if candidate.is_file(){return Some(candidate.to_string_lossy().into_owned());}
        #[cfg(windows)]{let exe=candidate.with_extension("exe");if exe.is_file(){return Some(exe.to_string_lossy().into_owned());}}}None}
fn versioned_bin_candidate(root:&Path,program:&str,prefix:Option<&str>)->Option<PathBuf>{let mut dirs=std::fs::read_dir(root).ok()?.filter_map(|e|e.ok()).map(|e|e.path()).filter(|p|p.is_dir()&&prefix.is_none_or(|prefix|p.file_name().and_then(|n|n.to_str()).is_some_and(|n|n.starts_with(prefix)))).collect::<Vec<_>>();dirs.sort();dirs.reverse();for dir in dirs{for bin in [dir.join("bin"),dir.clone()]{let candidate=bin.join(program);if candidate.is_file(){return Some(candidate);}}}None}
fn discover_host_executable(program:&str)->Option<PathBuf>{
    if let Some(path)=host_executable(program){return Some(PathBuf::from(path));}
    for root in ["/usr/lib/postgresql","/usr/lib64/postgresql","/usr/lib/postgresql-bin","/usr/lib64/postgresql-bin"]{if let Some(path)=versioned_bin_candidate(Path::new(root),program,None){return Some(path);}}
    for root in ["/usr/lib","/usr/lib64"]{if let Some(path)=versioned_bin_candidate(Path::new(root),program,Some("postgresql")){return Some(path);}}
    if let Some(path)=versioned_bin_candidate(Path::new("/opt"),program,None){return Some(path);}
    None
}
fn host_os_id()->String{std::fs::read_to_string("/etc/os-release").ok().and_then(|text|text.lines().find_map(|line|line.strip_prefix("ID=").map(|v|v.trim_matches('"').to_ascii_lowercase()))).unwrap_or_else(||"unknown".into())}
fn install_hint_for_os(os:&str,program:&str)->String{
    let group=if matches!(program,"initdb"|"postgres"|"pg_isready"|"createdb"){"postgresql"}else if matches!(program,"cargo"|"rustc"){"rust"}else{program};
    match os{"gentoo"=>format!("host tool is missing; inspect Gentoo packages with: emerge -s {group}"),"ubuntu"|"debian"=>format!("host tool is missing; inspect packages with: apt-cache search {group}"),"fedora"|"rhel"|"centos"=>format!("host tool is missing; inspect packages with: dnf search {group}"),"arch"|"manjaro"=>format!("host tool is missing; inspect packages with: pacman -Ss {group}"),_=>format!("host tool is missing; install a package providing {program} and retry")}
}
fn missing_program_details(programs:&[String],visible:&BTreeMap<String,String>)->Vec<Value>{let os=host_os_id();programs.iter().filter(|p|!visible.contains_key(*p)).map(|program|{let host=discover_host_executable(program);json!({"program":program,"state":if host.is_some(){"sandbox_missing"}else{"host_missing"},"host_path":host.as_ref().map(|p|p.to_string_lossy().into_owned()),"install_hint":host.is_none().then(||install_hint_for_os(&os,program))})}).collect()}
fn system_visible(path:&Path)->bool{["/usr","/bin","/sbin","/lib","/lib64"].iter().any(|root|path.starts_with(root))}
fn auto_mount_root(path:&Path)->Option<PathBuf>{let canonical=std::fs::canonicalize(path).ok()?;let mut components=canonical.components();match (components.next(),components.next()){(Some(std::path::Component::RootDir),Some(std::path::Component::Normal(first))) if first=="opt"=>components.next().and_then(|name|match name{std::path::Component::Normal(name)=>Some(Path::new("/opt").join(name)),_=>None}),_=>None}}
fn cargo_registry_under(home:&Path)->Option<PathBuf>{let registry=home.join(".cargo/registry");let metadata=std::fs::symlink_metadata(&registry).ok()?;if !metadata.is_dir()||metadata.file_type().is_symlink(){return None;}Some(registry)}
fn discover_host_cargo_registry()->Option<PathBuf>{std::env::var_os("HOME").and_then(|home|cargo_registry_under(Path::new(&home)))}
fn needs_cargo_registry(programs:&[String])->bool{programs.iter().any(|program|program=="cargo"||program=="rustc")}
fn execution_environment(config:&Config,programs:&[String])->(String,Vec<(PathBuf,PathBuf)>){
    let mut path_entries=std::env::split_paths(&config.execution.path).map(|p|p.to_string_lossy().into_owned()).collect::<Vec<_>>();
    let mut mounts=config.execution.readonly_mounts.iter().map(|m|(m.source.clone(),m.target.clone())).collect::<Vec<_>>();
    if config.execution.auto_discover_toolchains{
        for program in programs{
            let Some(host)=discover_host_executable(program) else{continue};
            let auto_root=auto_mount_root(&host);
            if let Some(parent)=host.parent(){let entry=parent.to_string_lossy().into_owned();if (system_visible(parent)||auto_root.is_some())&&!path_entries.iter().any(|p|p==&entry){path_entries.insert(0,entry);}}
            if let Some(root)=auto_root{if !mounts.iter().any(|(source,target)|source==&root&&target==&root){mounts.push((root.clone(),root));}}
        }
    }
    (path_entries.join(":"),mounts)
}
fn bubblewrap_base_command(config:&Config,programs:&[String],share_network:bool)->Result<(Command,String)>{
    if !cfg!(target_os="linux"){bail!("Bubblewrap is available only on Linux");}
    if !config.execution.bubblewrap.is_file(){bail!("bubblewrap is missing; install sys-apps/bubblewrap on Gentoo, or explicitly opt into unsafe host execution");}
    let (path,mounts)=execution_environment(config,programs);let mut c=Command::new(&config.execution.bubblewrap);clean_environment(&mut c,"/usr/bin:/bin");
    c.args(["--die-with-parent","--new-session","--unshare-all","--clearenv"]);
    if share_network{c.arg("--share-net");}
    for p in ["/usr","/bin","/sbin","/lib","/lib64"]{let p=Path::new(p);if p.exists(){c.arg("--ro-bind").arg(p).arg(p);}}
    c.args(["--proc","/proc","--dev","/dev","--tmpfs","/tmp","--dir","/tmp/home","--dir","/etc"]);
    for p in ["/etc/ld.so.cache","/etc/ld.so.conf","/etc/ld.so.conf.d","/etc/localtime"]{if Path::new(p).exists(){c.arg("--ro-bind").arg(p).arg(p);}}
    if share_network{for p in ["/etc/resolv.conf","/etc/hosts","/etc/ssl/certs"]{if Path::new(p).exists(){c.arg("--ro-bind").arg(p).arg(p);}}}
    for (source,target) in mounts{c.arg("--ro-bind").arg(source).arg(target);}
    Ok((c,path))
}
fn rust_toolchain_root()->Option<PathBuf>{
    let rustup=host_executable("rustup")?;let output=std::process::Command::new(rustup).args(["which","rustc"]).output().ok()?;if !output.status.success(){return None;}let rustc=PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());let bin=rustc.parent()?;if bin.file_name()? != "bin"{return None;}Some(bin.parent()?.to_owned())
}
fn toml_escape(value:&str)->String{value.replace('\\',"\\\\").replace('"',"\\\"")}
fn suggested_toml(current_path:&str,path_entries:&[String],mounts:&[Value])->String{
    let mut path=path_entries.to_vec();path.extend(current_path.split(':').filter(|entry|!entry.is_empty()&&!path_entries.iter().any(|p|p==entry)).map(str::to_owned));let mut text=format!("[execution]\npath = \"{}\"\n",toml_escape(&path.join(":")));
    if !mounts.is_empty(){text.push_str("readonly_mounts = [\n");for mount in mounts{if let (Some(source),Some(target))=(mount.get("source").and_then(Value::as_str),mount.get("target").and_then(Value::as_str)){text.push_str(&format!("  {{ source = \"{}\", target = \"{}\" }},\n",toml_escape(source),toml_escape(target)));}}text.push_str("]\n");}text
}
fn mount_suggestion(program:&str,host_path:&str)->Option<Value>{
    let path=std::fs::canonicalize(host_path).unwrap_or_else(|_|PathBuf::from(host_path));let bin=path.parent()?.to_owned();
    if system_visible(&bin){return Some(json!({"kind":"path-only","path_entry":bin,"reason":"host path is already covered by the sandbox system mounts"}));}
    if matches!(program,"cargo"|"rustc"){
        if let Some(root)=rust_toolchain_root(){return Some(json!({"kind":"readonly-mount","source":root,"target":"/opt/rust","path_entry":"/opt/rust/bin","reason":"mount only the active rustup toolchain root, not HOME or ~/.cargo"}));}
    }
    if matches!(program,"initdb"|"postgres"|"pg_isready"|"createdb"){
        let root=bin.parent().unwrap_or(&bin);return Some(json!({"kind":"readonly-mount","source":root,"target":"/opt/postgres","path_entry":"/opt/postgres/bin","reason":"mount only the PostgreSQL installation prefix; never mount its data directory or socket"}));
    }
    Some(json!({"kind":"readonly-mount","source":bin,"target":format!("/opt/{program}"),"path_entry":format!("/opt/{program}"),"reason":"review this minimal executable directory before adding it"}))
}
async fn probe_namespace(config:&Config,programs:&[String])->Result<()>{
    let (mut namespace,path)=bubblewrap_base_command(config,programs,false)?;
    namespace.args(["--setenv","HOME","/tmp/home","--setenv","PATH",&path,"--setenv","LANG","C.UTF-8","--setenv","LC_ALL","C.UTF-8","--setenv","TERM","dumb","--","/bin/sh","-c","exit 0"]);
    let result=capture(namespace,None,16384,5).await?;
    if result.code!=Some(0){bail!("bubblewrap sandbox probe failed: {}",crate::util::bounded_text(&String::from_utf8_lossy(&result.stderr),2048));}Ok(())
}
async fn sandbox_paths(config:&Config,programs:&[String])->Result<BTreeMap<String,String>>{
    probe_namespace(config,programs).await?;let (mut probe,path)=bubblewrap_base_command(config,programs,false)?;
    probe.args(["--setenv","HOME","/tmp/home","--setenv","PATH",&path,"--setenv","LANG","C.UTF-8","--setenv","LC_ALL","C.UTF-8","--setenv","TERM","dumb","--","/bin/sh","-c","for p do if v=$(command -v \"$p\" 2>/dev/null); then printf 'OK\\t%s\\t%s\\n' \"$p\" \"$v\"; else printf 'MISSING\\t%s\\n' \"$p\"; fi; done","probe"]);probe.args(programs);
    let result=capture(probe,None,262144,10).await?;if result.code!=Some(0){bail!("bubblewrap program visibility probe failed: {}",crate::util::bounded_text(&String::from_utf8_lossy(&result.stderr),2048));}let stdout=String::from_utf8(result.stdout).context("Sandbox toolchain probe output is not UTF-8")?;let mut visible=BTreeMap::new();for line in stdout.lines(){let mut fields=line.split('\t');if fields.next()==Some("OK"){if let (Some(program),Some(path))=(fields.next(),fields.next()){visible.insert(program.to_owned(),path.to_owned());}}}Ok(visible)
}
pub async fn sandbox_diagnostics(config:&Config)->Result<Value>{
    let programs=diagnostic_programs(config);let (effective_path,effective_mounts)=execution_environment(config,&programs);let sandbox=if config.execution.backend=="bubblewrap"{sandbox_paths(config,&programs).await?}else{BTreeMap::new()};let os=host_os_id();let mut suggested_paths=Vec::<String>::new();let mut suggested_mounts=Vec::<Value>::new();let rows=programs.iter().map(|program|{let host=discover_host_executable(program);let host_path=host.as_ref().map(|p|p.to_string_lossy().into_owned());let sandbox_path=sandbox.get(program).cloned();let status=if sandbox_path.is_some(){"available"}else if host_path.is_some(){"host-only"}else{"missing"};let suggestion=if status=="host-only"{host_path.as_deref().and_then(|path|mount_suggestion(program,path))}else{None};if let Some(value)=&suggestion{if let Some(path)=value.get("path_entry").and_then(Value::as_str){if !effective_path.split(':').any(|entry|entry==path)&&!suggested_paths.iter().any(|entry|entry==path){suggested_paths.push(path.to_owned());}}if value.get("kind").and_then(Value::as_str)==Some("readonly-mount"){let source=value.get("source").cloned().unwrap_or(Value::Null);let target=value.get("target").cloned().unwrap_or(Value::Null);if !suggested_mounts.iter().any(|m|m.get("source")==Some(&source)&&m.get("target")==Some(&target)){suggested_mounts.push(json!({"source":source,"target":target}));}}}json!({"program":program,"status":status,"reason":if sandbox_path.is_some(){Value::Null}else if host_path.is_some(){json!("sandbox_missing")}else{json!("host_missing")},"install_hint":if host_path.is_none(){json!(install_hint_for_os(&os,program))}else{Value::Null},"allowed":config.execution.allowed_programs.contains(program),"required":config.execution.required_programs.contains(program),"host_path":host_path,"sandbox_path":sandbox_path,"suggestion":suggestion})}).collect::<Vec<_>>();let mounts=effective_mounts.iter().map(|(source,target)|json!({"source":source,"target":target,"source_exists":source.exists(),"target_allowed":target.starts_with("/opt/")||target.starts_with("/cache-readonly/"),"automatic":!config.execution.readonly_mounts.iter().any(|m|m.source==*source&&m.target==*target)})).collect::<Vec<_>>();let toml=suggested_toml(&effective_path,&suggested_paths,&suggested_mounts);Ok(json!({"backend":config.execution.backend,"host_os":os,"path":effective_path,"configured_path":config.execution.path,"auto_discover_toolchains":config.execution.auto_discover_toolchains,"namespace_ok":config.execution.backend=="bubblewrap","programs":rows,"readonly_mounts":mounts,"suggested_config":{"prepend_path_entries":suggested_paths,"readonly_mounts":suggested_mounts,"toml":toml},"network":config.execution.allow_network,"shell":config.execution.allow_shell}))
}
fn job_preflight_programs(program:&str,requested:&[String],shell:bool)->Result<Vec<String>>{if requested.len()>32{bail!("preflight_programs accepts at most 32 program names");}let mut programs=requested.to_vec();if !shell{programs.push(program.to_owned());if program=="cargo"{programs.push("rustc".into());}}for item in &programs{if !simple_program(item){bail!("preflight_programs must contain simple executable names");}}programs.sort();programs.dedup();Ok(programs)}
pub async fn preflight_job(config:&Config,program:&str,requested:&[String],shell:bool)->Result<Value>{
    let programs=job_preflight_programs(program,requested,shell)?;if programs.is_empty(){return Ok(json!({"backend":config.execution.backend,"programs":[],"toolchains":{"rust":{"requested":false,"available":false},"postgresql":{"requested":false,"available":false}}}));}
    let visible:BTreeMap<String,String>=match config.execution.backend.as_str(){"bubblewrap"=>sandbox_paths(config,&programs).await?,"host"=>programs.iter().filter_map(|p|configured_executable(&config.execution.path,p).map(|path|(p.clone(),path.to_string_lossy().into_owned()))).collect(),"disabled"=>return Err(crate::error::coded("JOB_PREFLIGHT_FAILED",false,"execution backend is disabled")),_=>return Err(crate::error::coded("JOB_PREFLIGHT_FAILED",false,"unknown execution backend"))};
    let missing=programs.iter().filter(|p|!visible.contains_key(*p)).cloned().collect::<Vec<_>>();if !missing.is_empty(){let details=missing_program_details(&programs,&visible);let host_missing=details.iter().filter_map(|v|(v["state"]=="host_missing").then(||v["program"].as_str().unwrap_or("").to_owned())).collect::<Vec<_>>();let sandbox_missing=details.iter().filter_map(|v|(v["state"]=="sandbox_missing").then(||v["program"].as_str().unwrap_or("").to_owned())).collect::<Vec<_>>();let (effective_path,_)=execution_environment(config,&programs);return Err(crate::error::coded_details("JOB_PREFLIGHT_FAILED",false,format!("required job program(s) unavailable: {}",missing.join(", ")),json!({"backend":config.execution.backend,"missing":missing,"host_missing":host_missing,"sandbox_missing":sandbox_missing,"programs":details,"path":effective_path,"requested":programs,"auto_discover_toolchains":config.execution.auto_discover_toolchains})));}
    let rows=programs.iter().map(|p|json!({"program":p,"path":visible.get(p)})).collect::<Vec<_>>();let rust_requested=programs.iter().any(|p|matches!(p.as_str(),"cargo"|"rustc"));let postgres_names=["initdb","postgres","pg_isready","createdb"];let postgres_requested=programs.iter().any(|p|postgres_names.contains(&p.as_str()));let postgres_paths=postgres_names.iter().map(|p|((*p).to_owned(),visible.get(*p).cloned())).collect::<BTreeMap<_,_>>();Ok(json!({"backend":config.execution.backend,"programs":rows,"toolchains":{"rust":{"requested":rust_requested,"available":rust_requested&&visible.contains_key("cargo")&&visible.contains_key("rustc"),"cargo_path":visible.get("cargo"),"rustc_path":visible.get("rustc")},"postgresql":{"requested":postgres_requested,"available":postgres_requested&&postgres_names.iter().all(|p|visible.contains_key(*p)),"paths":postgres_paths}}}))
}

pub async fn probe_bubblewrap(config:&Config)->Result<Vec<(String,String)>>{
    if config.execution.backend!="bubblewrap"{bail!("Configured execution backend is not bubblewrap");}
    let programs=configured_probe_programs(config);let visible=sandbox_paths(config,&programs).await?;let missing=programs.iter().filter(|p|!visible.contains_key(*p)).cloned().collect::<Vec<_>>();
    if !missing.is_empty(){let required_missing=missing.iter().filter(|p|config.execution.required_programs.contains(*p)).cloned().collect::<Vec<_>>();bail!("bubblewrap sandbox probe: configured program(s) not visible: {}. required_missing=[{}]. execution.path={}. Mount only minimal toolchain prefixes read-only (for example Rust to /opt/rust and PostgreSQL to /opt/postgres), prepend their bin directories to execution.path, and never mount HOME, database data directories, sockets, Docker socket or credential directories",missing.join(", "),required_missing.join(", "),config.execution.path);}
    Ok(visible.into_iter().collect())
}

fn git_network_subcommand(args:&[String])->bool{args.iter().any(|arg|matches!(arg.as_str(),"push"|"fetch"|"pull"|"clone"|"ls-remote"|"remote"|"submodule"))}
fn git_identity(config:&Config)->[(&'static str,&str);4]{[("GIT_AUTHOR_NAME",&config.git.author_name),("GIT_AUTHOR_EMAIL",&config.git.author_email),("GIT_COMMITTER_NAME",&config.git.author_name),("GIT_COMMITTER_EMAIL",&config.git.author_email)]}
fn sandbox_git_identity(command:&mut Command,config:&Config){for(name,value)in git_identity(config){command.args(["--setenv",name,value]);}}
pub fn build_job_command(config:&Config,w:&Project,program:&str,args:&[String],cwd:&str,shell:bool,project_programs:&[String],environment:&BTreeMap<String,String>,network:bool,summary_path:&str)->Result<Command>{
    w.exec_allowed()?;validate_job_environment(environment)?;validate_job_network(config,w.config.development(),network)?;
    if program=="git"&&git_network_subcommand(args){bail!("Network Git subcommands are disabled in run_command; allow_git_mutation only permits local repository mutations");}
    if args.len()>128||args.iter().any(|a|a.contains('\0')||a.len()>65536)||args.iter().map(|a|a.len()).sum::<usize>()>131072{bail!("Command arguments exceed limits");}
    if shell&&!config.execution.allow_shell{bail!("run_shell is disabled; set execution.allow_shell=true locally after reviewing the risks");}
    // trusted_host is a per-Project opt-in for native host executables, not a global wildcard.
    // It is deliberately unavailable in bubblewrap or without explicit host acknowledgement.
    let trusted_host=w.config.trusted_host()&&config.execution.backend=="host"&&config.execution.acknowledge_unsafe_host_execution;
    if w.config.trusted_host()&&!trusted_host{bail!("trusted_host requires explicitly acknowledged host execution");}
    if !shell&&!trusted_host&&!config.execution.allowed_programs.iter().any(|p|p==program)&&!project_programs.iter().any(|p|p==program){bail!("Program is neither globally allowed nor detected from this project's toolchain manifests");}
    let host_cwd=w.root.directory_path(cwd)?;
    let mut command=match config.execution.backend.as_str(){
        "disabled"=>bail!("Command execution backend is disabled"),
        "host"=>{
            if !config.execution.acknowledge_unsafe_host_execution{bail!("Host execution has not been explicitly authorized");}
            let executable=if shell{validated_executable(&config.execution.path,if cfg!(windows){"powershell"}else{"bash"})?}else{validated_executable(&config.execution.path,program)?};
            let mut c=Command::new(executable);clean_environment(&mut c,&config.execution.path);c.current_dir(host_cwd);
            // Explicit host mode is not a sandbox; no service token/key is inherited.
            if let Some(home)=std::env::var_os("HOME").or_else(||std::env::var_os("USERPROFILE")){c.env("HOME",home);}c.envs(environment).env("ENDLESSVIBE_JOB_SUMMARY",summary_path);c.args(args);c
        }
        "bubblewrap"=>{
            let cache=config.security.data_dir.join("exec-cache").join(&w.workspace_id).join(&w.config.id);crate::util::private_dir(&cache)?;
            let mut programs=configured_probe_programs(config);programs.extend(project_programs.iter().cloned());programs.sort();programs.dedup();let (mut c,path)=bubblewrap_base_command(config,&programs,network)?;
            c.arg("--bind").arg(&w.root.path).arg("/workspace").arg("--bind").arg(&cache).arg("/cache");
            let docker_socket=if config.docker.allow_project_socket&&w.config.development()&&network&&!environment.contains_key("DOCKER_HOST"){discover_docker_socket()}else{None};if let Some(socket)=&docker_socket{c.args(["--dir","/run","--dir","/run/endlessvibe"]).arg("--bind").arg(socket).arg("/run/endlessvibe/docker.sock");}
            if config.execution.auto_discover_toolchains&&needs_cargo_registry(&programs){if let Some(registry)=discover_host_cargo_registry(){let target=cache.join("cargo/registry");crate::util::private_dir(&target)?;c.arg("--ro-bind").arg(registry).arg("/cache/cargo/registry");}}
            // Git metadata is read-only by default. Explicit allow_git_mutation keeps the project's
            // own .git writable while HOME, credentials, service state and network remain isolated.
            let git_metadata=w.root.path.join(".git");if git_metadata.exists(){if std::fs::symlink_metadata(&git_metadata)?.file_type().is_symlink(){bail!("Sandbox refuses symlinked Git metadata");}if !w.config.allow_git_mutation{c.arg("--ro-bind").arg(&git_metadata).arg("/workspace/.git");}}
            let inside=if cwd=="."{PathBuf::from("/workspace")}else{Path::new("/workspace").join(cwd)};
            c.arg("--chdir").arg(inside).args(["--setenv","HOME","/tmp/home","--setenv","PATH",&path,"--setenv","CARGO_HOME","/cache/cargo","--setenv","XDG_CACHE_HOME","/cache/xdg","--setenv","LANG","C.UTF-8","--setenv","LC_ALL","C.UTF-8","--setenv","TERM","dumb","--setenv","ENDLESSVIBE_JOB_SUMMARY",summary_path]);
            for(name,value)in environment{c.arg("--setenv").arg(name).arg(value);}
            // Bubblewrap clears the launcher environment: inject the configured Git identity inside the sandbox.
            // Apply it to all jobs so nested Git commands (for example, from Python or build scripts) agree with git_commit.
            sandbox_git_identity(&mut c,config);
            if !shell&&program=="git"{c.args(["--setenv","GIT_MERGE_AUTOEDIT","no","--setenv","GIT_EDITOR","true"]);}
            if docker_socket.is_some(){c.args(["--setenv","DOCKER_HOST","unix:///run/endlessvibe/docker.sock"]);}
            c.arg("--").arg(if shell{"/bin/bash"}else{program}).args(args);c
        }
        _=>bail!("Unknown execution backend"),
    };
    if config.execution.backend=="host"{for(name,value)in git_identity(config){command.env(name,value);}}
    if !shell&&program=="git"&&config.execution.backend=="host"{command.env("GIT_MERGE_AUTOEDIT","no").env("GIT_EDITOR","true");}
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]command.process_group(0);
    #[cfg(unix)]{
        let address_space=config.execution.memory_limit_mb.saturating_mul(1024*1024);
        let processes=pre_exec_nproc_limit(&config.execution.backend,config.execution.max_processes);
        let cpu=config.limits.command_timeout_seconds+5;
        // SAFETY: only async-signal-safe syscalls are used in this pre_exec closure.
        unsafe{command.pre_exec(move||{
            for (resource,value) in [(libc::RLIMIT_AS,address_space),(libc::RLIMIT_CPU,cpu)]{
                let lim=libc::rlimit{rlim_cur:value as libc::rlim_t,rlim_max:value as libc::rlim_t};
                if libc::setrlimit(resource,&lim)!=0{return Err(std::io::Error::last_os_error());}
            }
            if let Some(value)=processes{
                let lim=libc::rlimit{rlim_cur:value as libc::rlim_t,rlim_max:value as libc::rlim_t};
                if libc::setrlimit(libc::RLIMIT_NPROC,&lim)!=0{return Err(std::io::Error::last_os_error());}
            }
            #[cfg(target_os="linux")]
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS,1 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong,0 as libc::c_ulong)!=0{return Err(std::io::Error::last_os_error());}
            Ok(())
        });}
    }
    Ok(command)
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn bubblewrap_does_not_limit_launcher_nproc(){assert_eq!(pre_exec_nproc_limit("bubblewrap",256),None);}
    #[test]fn host_keeps_configured_nproc_limit(){assert_eq!(pre_exec_nproc_limit("host",256),Some(256));}
    #[test]
    fn trusted_host_can_run_installed_programs_without_global_allowlisting(){
        let temp=tempfile::tempdir().unwrap();
        let base=crate::security::paths::Root::open(temp.path()).unwrap();
        let mut config=Config::default();
        config.execution.backend="host".into();
        config.execution.acknowledge_unsafe_host_execution=true;
        config.execution.path="/usr/bin:/bin".into();
        config.execution.allow_shell=false;
        let mut project=Project{
            workspace_id:"demo".into(),
            config:crate::config::ProjectConfig{id:"kernel".into(),path:".".into(),
                allow_write:true,allow_exec:true,allow_git_commit:false,
                allow_git_mutation:false,allow_git_push:false,
                execution_profile:"development".into(),environment:vec![]},
            root:base,
            lock:std::sync::Arc::new(tokio::sync::Mutex::new(()))
        };
        let env=BTreeMap::new();
        assert!(!config.execution.allowed_programs.iter().any(|v|v=="sh"));
        assert!(build_job_command(&config,&project,"sh",&["-c".into(),"true".into()],
            ".",false,&[],&env,true,"/tmp/job-summary.json").is_err());
        project.config.execution_profile="trusted_host".into();
        assert!(config.validate().is_ok());
        assert!(build_job_command(&config,&project,"sh",&["-c".into(),"true".into()],
            ".",false,&[],&env,true,"/tmp/job-summary.json").is_ok());
        assert!(build_job_command(&config,&project,"not-a-real-program", &[],
            ".",false,&[],&env,true,"/tmp/job-summary.json").is_err());
        config.execution.backend="bubblewrap".into();
        assert!(config.validate().is_ok()); // No Project registered; direct call still fails closed.
        assert!(build_job_command(&config,&project,"sh",&[],
            ".",false,&[],&env,true,"/tmp/job-summary.json").is_err());
    }
    #[test]fn required_programs_do_not_grant_run_permission(){let mut c=Config::default();c.execution.required_programs=vec!["postgres".into()];assert!(!c.execution.allowed_programs.contains(&"postgres".to_owned()));}
    #[test]fn diagnostics_always_include_joint_gate_tools(){let c=Config::default();let programs=diagnostic_programs(&c);for program in DIAGNOSTIC_PROGRAMS{assert!(programs.contains(&program.to_owned()));}}
    #[test]fn system_tools_only_need_path_suggestion(){let v=mount_suggestion("postgres","/usr/bin/postgres").unwrap();assert_eq!(v["kind"],"path-only");assert_eq!(v["path_entry"],"/usr/bin");}
    #[test]fn cargo_registry_discovery_exposes_only_registry_subtree(){let t=tempfile::tempdir().unwrap();let cargo=t.path().join(".cargo");let registry=cargo.join("registry");std::fs::create_dir_all(&registry).unwrap();std::fs::write(cargo.join("credentials.toml"),"secret").unwrap();assert_eq!(cargo_registry_under(t.path()),Some(registry));assert!(needs_cargo_registry(&["cargo".into()]));assert!(needs_cargo_registry(&["rustc".into()]));assert!(!needs_cargo_registry(&["python3".into()]));}
    #[cfg(unix)]#[test]fn cargo_registry_discovery_rejects_symlink_root(){use std::os::unix::fs::symlink;let t=tempfile::tempdir().unwrap();let real=t.path().join("real");std::fs::create_dir_all(&real).unwrap();std::fs::create_dir_all(t.path().join(".cargo")).unwrap();symlink(&real,t.path().join(".cargo/registry")).unwrap();assert_eq!(cargo_registry_under(t.path()),None);}
    #[test]fn postgres_prefix_gets_minimal_mount(){let v=mount_suggestion("postgres","/opt/pgsql/bin/postgres").unwrap();assert_eq!(v["kind"],"readonly-mount");assert_eq!(v["source"],"/opt/pgsql");assert_eq!(v["target"],"/opt/postgres");assert_eq!(v["path_entry"],"/opt/postgres/bin");}
    #[test]fn suggested_toml_is_copyable_and_preserves_existing_path(){let mounts=vec![json!({"source":"/host/rust","target":"/opt/rust"})];let text=suggested_toml("/usr/bin:/bin",&["/opt/rust/bin".into()],&mounts);assert!(text.contains("path = \"/opt/rust/bin:/usr/bin:/bin\""));assert!(text.contains("source = \"/host/rust\", target = \"/opt/rust\""));}
    #[test]fn cargo_preflight_also_requires_rustc(){assert_eq!(job_preflight_programs("cargo",&[],false).unwrap(),vec!["cargo".to_owned(),"rustc".to_owned()]);}
    #[test]fn shell_preflight_is_explicit(){assert_eq!(job_preflight_programs("bash",&["postgres".into()],true).unwrap(),vec!["postgres".to_owned()]);}
    #[test]fn summary_contract_uses_private_cache_path(){let d=tempfile::tempdir().unwrap();let mut c=Config::default();c.security.data_dir=d.path().to_owned();let contract=job_summary_contract(&c,"root","demo","abc-123").unwrap();assert!(contract.host_path.starts_with(d.path().join("exec-cache/root/demo")));assert_eq!(contract.exposed_path,"/cache/job-summary-abc-123.json");}
    #[test]fn network_git_subcommands_are_blocked(){for sub in ["push","fetch","pull","clone","ls-remote","remote","submodule"]{assert!(git_network_subcommand(&[sub.into()]));}for sub in ["status","switch","merge","branch","add","commit","rebase"]{assert!(!git_network_subcommand(&[sub.into()]));}}
    #[test]fn configured_git_identity_is_injected_into_sandbox(){let mut cfg=Config::default();cfg.git.author_name="MRsoymilk".into();cfg.git.author_email="codermrsoymilk@gmail.com".into();let mut command=Command::new("/usr/bin/bwrap");sandbox_git_identity(&mut command,&cfg);let args=command.as_std().get_args().map(|v|v.to_string_lossy().into_owned()).collect::<Vec<_>>();assert_eq!(args,vec!["--setenv","GIT_AUTHOR_NAME","MRsoymilk","--setenv","GIT_AUTHOR_EMAIL","codermrsoymilk@gmail.com","--setenv","GIT_COMMITTER_NAME","MRsoymilk","--setenv","GIT_COMMITTER_EMAIL","codermrsoymilk@gmail.com"]);}
    #[test]fn project_manifest_detection_finds_joint_toolchains(){let d=tempfile::tempdir().unwrap();std::fs::write(d.path().join("Cargo.toml"),"[dependencies]\nsqlx = \"1\"\n").unwrap();std::fs::create_dir_all(d.path().join("game")).unwrap();std::fs::write(d.path().join("game/project.godot"),"[application]\n").unwrap();let mut programs=Vec::new();inspect_project_dir(d.path(),0,&mut programs);for p in ["cargo","rustc","initdb","postgres","pg_isready","createdb","godot"]{assert!(programs.contains(&p.to_owned()),"missing {p}");}}
    #[test]fn oversized_manifest_is_not_read(){let d=tempfile::tempdir().unwrap();let p=d.path().join("Cargo.toml");std::fs::write(&p,vec![b'x';512*1024+1]).unwrap();assert!(read_small_manifest(&p).is_none());}
    #[test]fn service_program_mentions_are_advisory_without_required_marker(){let mut p=Vec::new();scan_program_mentions("subprocess.run([\"cargo\"]); LOCAL=(\"initdb\", \"postgres\")",&mut p);for expected in ["cargo","rustc"]{assert!(p.contains(&expected.to_owned()),"missing {expected}");}for unexpected in ["initdb","postgres","pg_isready","createdb"]{assert!(!p.contains(&unexpected.to_owned()),"unexpected {unexpected}");}}
    #[test]fn required_program_marker_can_gate_local_services(){let mut p=Vec::new();scan_program_mentions("# endlessvibe-required-programs: initdb postgres pg_isready createdb\nLOCAL=(\"initdb\", \"postgres\")",&mut p);for expected in ["initdb","postgres","pg_isready","createdb"]{assert!(p.contains(&expected.to_owned()),"missing {expected}");}}
    #[test]fn script_optional_programs_do_not_block_preflight(){let mut p=Vec::new();scan_program_mentions("# endlessvibe-optional-programs: initdb postgres\nsubprocess.run([\"cargo\"]); LOCAL=(\"initdb\", \"postgres\")",&mut p);assert!(p.contains(&"cargo".to_owned()));assert!(p.contains(&"rustc".to_owned()));assert!(!p.contains(&"initdb".to_owned()));assert!(!p.contains(&"postgres".to_owned()));}
    #[test]fn python_compile_modes_skip_runtime_tool_scan(){assert!(python_compile_only(&["-m".into(),"py_compile".into(),"tools/check.py".into()]));assert!(python_compile_only(&["-m".into(),"compileall".into(),"tools".into()]));assert!(!python_compile_only(&["tools/check.py".into()]));}
    #[test]fn gentoo_install_hint_is_search_only(){let hint=install_hint_for_os("gentoo","postgres");assert!(hint.contains("emerge -s postgresql"));assert!(!hint.contains("emerge -av"));}
    #[test]fn job_environment_accepts_external_service_variables_and_reserves_runtime_keys(){let mut env=BTreeMap::new();env.insert("TEST_DATABASE_URL".into(),"postgres://user:password@127.0.0.1:19031/test".into());assert!(validate_job_environment(&env).is_ok());env.insert("PATH".into(),"/tmp".into());assert!(validate_job_environment(&env).is_err());let mut invalid=BTreeMap::new();invalid.insert("1BAD".into(),"x".into());assert!(validate_job_environment(&invalid).is_err());}
    #[test]fn bubblewrap_network_is_per_job_and_project_profile(){let mut c=Config::default();assert!(validate_job_network(&c,false,false).is_ok());assert!(validate_job_network(&c,false,true).is_err());assert!(validate_job_network(&c,true,true).is_ok());c.execution.allow_network=true;assert!(validate_job_network(&c,false,true).is_ok());}
    #[test]fn project_docker_socket_access_requires_separate_opt_in(){let c=Config::default();assert!(!c.docker.allow_project_socket);assert!(!c.docker.enabled);let mut d=c.clone();d.docker.enabled=true;d.docker.allowed_containers.push("endlessvibe".into());assert!(!d.docker.allow_project_socket);#[cfg(unix)]assert!(d.validate().is_ok());#[cfg(windows)]assert!(d.validate().is_err());}
    #[cfg(unix)]
    #[test]fn docker_socket_detection_rejects_regular_files(){let d=tempfile::tempdir().unwrap();let regular=d.path().join("docker.sock");std::fs::write(&regular,b"not a socket").unwrap();assert!(existing_unix_socket(&regular).is_none());let socket=d.path().join("real.sock");let _listener=std::os::unix::net::UnixListener::bind(&socket).unwrap();assert_eq!(existing_unix_socket(&socket),Some(socket));}
}
