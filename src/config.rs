use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap,HashSet}, env, net::SocketAddr, path::{Path, PathBuf}};
use url::Url;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config { pub server: Server, pub security: Security, pub limits: Limits, pub execution: Execution, pub git: Git, pub docker: Docker, pub transfer:Transfer, pub workspaces: Vec<WorkspaceConfig> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server { pub bind: SocketAddr, pub public_url: String, pub allowed_hosts: Vec<String>, pub lan_only: bool }
// The public origin is intentionally a non-operational example, never an owner-specific domain.
impl Default for Server { fn default() -> Self { Self { bind: "0.0.0.0:20000".parse().unwrap(), public_url: "https://mcp.example.com".into(), lan_only: false, allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "host.docker.internal".into(), "endlessvibe".into()] } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Security { pub data_dir: PathBuf, pub allow_http_loopback: bool, pub extra_redirect_uris: Vec<String>, pub access_token_seconds: u64, pub refresh_token_seconds: u64, pub allow_windows_service_update: bool, pub windows_update_project: Option<PathBuf> }
impl Default for Security { fn default() -> Self { Self { data_dir: default_state_dir(), allow_http_loopback: false, extra_redirect_uris: vec![], access_token_seconds: 3600, refresh_token_seconds: 30 * 86400, allow_windows_service_update:false, windows_update_project:None } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits { pub max_file_bytes: usize, pub max_read_bytes: usize, pub max_output_bytes: usize, pub command_timeout_seconds: u64, pub max_jobs: usize, pub retained_jobs: usize, pub search_max_files: usize, pub search_max_bytes: usize }
impl Default for Limits { fn default() -> Self { Self { max_file_bytes: 2 * 1024 * 1024, max_read_bytes: 256 * 1024, max_output_bytes: 1024 * 1024, command_timeout_seconds: 120, max_jobs: 2, retained_jobs: 100, search_max_files: 5000, search_max_bytes: 64 * 1024 * 1024 } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Execution { pub backend: String, pub acknowledge_unsafe_host_execution: bool, pub allow_shell: bool, pub allow_network: bool, pub auto_discover_toolchains: bool, pub bubblewrap: PathBuf, pub path: String, pub allowed_programs: Vec<String>, pub required_programs: Vec<String>, pub readonly_mounts: Vec<ReadOnlyMount>, pub memory_limit_mb: u64, pub max_processes: u64 }
impl Default for Execution { fn default() -> Self { Self { backend: if cfg!(target_os="linux"){"bubblewrap".into()}else{"disabled".into()}, acknowledge_unsafe_host_execution: false, allow_shell: false, allow_network: false, auto_discover_toolchains: true, bubblewrap: "/usr/bin/bwrap".into(), path: default_execution_path(), allowed_programs: ["cargo", "rustc", "cmake", "ninja", "make", "ctest", "python3", "git", "rg"].map(str::to_owned).to_vec(), required_programs: vec![], readonly_mounts: vec![], memory_limit_mb: 8192, max_processes: 256 } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyMount { pub source: PathBuf, pub target: PathBuf }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Git { pub executable: PathBuf, pub author_name: String, pub author_email: String }
impl Default for Git { fn default() -> Self { Self { executable: if cfg!(windows){"git".into()}else{"/usr/bin/git".into()}, author_name: "EndlessVibe".into(), author_email: "endlessvibe@localhost".into() } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Docker { pub enabled:bool, pub socket:PathBuf, pub allowed_containers:Vec<String>, pub allow_start:bool, pub allow_stop:bool, pub allow_restart:bool, pub allow_project_socket:bool }
impl Default for Docker { fn default()->Self{Self{enabled:false,socket:if cfg!(windows){PathBuf::new()}else{"/var/run/docker.sock".into()},allowed_containers:vec![],allow_start:false,allow_stop:false,allow_restart:false,allow_project_socket:false}} }
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(default,deny_unknown_fields)]
pub struct Transfer{pub enabled:bool,pub listen:SocketAddr,pub advertise:bool,pub discover:bool,pub display_name:String}
impl Default for Transfer{fn default()->Self{Self{enabled:false,listen:"0.0.0.0:20002".parse().unwrap(),advertise:true,discover:true,display_name:"EndlessVibe".into()}}}
pub fn valid_docker_container(s:&str)->bool{!s.is_empty()&&s.len()<=128&&s.bytes().next().is_some_and(|b|b.is_ascii_alphanumeric())&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b"_.-".contains(&b))}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig { pub id: String, pub path: PathBuf, #[serde(default, skip_serializing_if = "Vec::is_empty")] pub projects: Vec<ProjectConfig>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_write: Option<bool>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_exec: Option<bool>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_git_commit: Option<bool>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_git_mutation: Option<bool>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_git_push: Option<bool>, #[serde(default,skip_serializing_if="Option::is_none")] pub execution_profile:Option<String>, #[serde(default,skip_serializing_if="Vec::is_empty")] pub environment:Vec<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig { pub id: String, pub path: PathBuf, #[serde(default)] pub allow_write: bool, #[serde(default)] pub allow_exec: bool, #[serde(default)] pub allow_git_commit: bool, #[serde(default)] pub allow_git_mutation: bool, #[serde(default)] pub allow_git_push: bool, #[serde(default="default_project_profile",skip_serializing_if="is_isolated_profile")] pub execution_profile:String, #[serde(default,skip_serializing_if="Vec::is_empty")] pub environment:Vec<String> }

pub fn default_project_profile()->String{"isolated".into()}
fn is_isolated_profile(value:&String)->bool{value=="isolated"}
pub fn valid_environment_name(name:&str)->bool{let mut bytes=name.bytes();matches!(bytes.next(),Some(b)if b.is_ascii_alphabetic()||b==b'_')&&bytes.all(|b|b.is_ascii_alphanumeric()||b==b'_')}
pub fn parse_environment_entries(entries:&[String])->Result<BTreeMap<String,String>>{if entries.len()>64{bail!("project environment accepts at most 64 entries");}let reserved=["HOME","PATH","CARGO_HOME","XDG_CACHE_HOME","LANG","LC_ALL","TERM","ENDLESSVIBE_JOB_SUMMARY"];let mut out=BTreeMap::new();let mut total=0usize;for entry in entries{let(name,value)=entry.split_once('=').context("project environment entries must use NAME=VALUE")?;if !valid_environment_name(name)||name.len()>128{bail!("project environment names must match [A-Za-z_][A-Za-z0-9_]* and be at most 128 bytes");}if reserved.contains(&name){bail!("project environment variable {name} is reserved by EndlessVibe");}if value.contains('\0')||value.len()>65536{bail!("project environment variable {name} contains NUL or exceeds 65536 bytes");}total=total.saturating_add(entry.len());if out.insert(name.to_owned(),value.to_owned()).is_some(){bail!("project environment contains duplicate variable {name}");}}if total>131072{bail!("project environment exceeds 131072 bytes");}Ok(out)}
impl ProjectConfig{pub fn development(&self)->bool{matches!(self.execution_profile.as_str(),"development"|"trusted_host")}pub fn trusted_host(&self)->bool{self.execution_profile=="trusted_host"}pub fn environment_map(&self)->Result<BTreeMap<String,String>>{parse_environment_entries(&self.environment)}}

#[cfg(target_os="linux")]
pub fn default_config_path() -> PathBuf { env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("endlessvibe/config.toml") }
#[cfg(target_os="linux")]
pub fn default_state_dir() -> PathBuf { env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state")).join("endlessvibe") }
#[cfg(target_os="macos")]
pub fn default_config_path() -> PathBuf { env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join("Library/Application Support")).join("EndlessVibe/config.toml") }
#[cfg(target_os="macos")]
pub fn default_state_dir() -> PathBuf { env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join("Library/Application Support")).join("EndlessVibe/state") }
#[cfg(windows)]
pub fn default_config_path() -> PathBuf { env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData/Roaming")).join("EndlessVibe/config.toml") }
#[cfg(windows)]
pub fn default_state_dir() -> PathBuf { env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData/Local")).join("EndlessVibe") }
fn home() -> PathBuf { env::var_os("HOME").or_else(||env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }
// Never copy the process PATH unvalidated into the first-run configuration.
// Windows PATH can contain empty, relative, drive-relative and unexpanded
// entries that are legal in a login shell but forbidden in an execution policy.
#[cfg(any(windows,test))]
fn inherited_absolute_path_entries(raw:&std::ffi::OsStr)->Vec<PathBuf>{
    let mut entries=Vec::<PathBuf>::new();
    for entry in env::split_paths(raw){
        if !entry.is_absolute()
            ||entry.components().any(|part|matches!(part,std::path::Component::ParentDir))
            ||!entry.to_str().is_some_and(|text|!text.contains(['"', '%', '\0']))
            ||env::join_paths([entry.as_os_str()]).is_err(){
            continue;
        }
        if !entries.iter().any(|prior|if cfg!(windows){
            prior.to_string_lossy().eq_ignore_ascii_case(&entry.to_string_lossy())
        }else{prior==&entry}){
            entries.push(entry);
        }
    }
    entries
}
#[cfg(windows)]
fn windows_default_execution_path(raw:Option<&std::ffi::OsStr>,system_root:Option<&std::ffi::OsStr>)->String{
    let root=system_root.map(PathBuf::from)
        .filter(|path|path.is_absolute()
            &&!path.components().any(|part|matches!(part,std::path::Component::ParentDir))
            &&path.to_str().is_some_and(|text|!text.contains(['"', '%', '\0'])))
        .unwrap_or_else(||PathBuf::from(r"C:\Windows"));
    // Keep known OS locations first. This makes a copied EXE usable even when
    // the target account has a truncated or malformed PATH.
    let mut entries=vec![root.join("System32"),root];
    if let Some(value)=raw{
        for entry in inherited_absolute_path_entries(value){
            if !entries.iter().any(|old|old.to_string_lossy().eq_ignore_ascii_case(&entry.to_string_lossy())){
                entries.push(entry);
            }
        }
    }
    env::join_paths(&entries).ok().and_then(|value|value.into_string().ok())
        .unwrap_or_else(||r"C:\Windows\System32;C:\Windows".into())
}
fn default_execution_path()->String{
    #[cfg(windows)]
    {return windows_default_execution_path(env::var_os("PATH").as_deref(),env::var_os("SystemRoot").as_deref());}
    #[cfg(not(windows))]
    {"/usr/local/bin:/usr/bin:/bin".into()}
}
/// Accept a domain or HTTPS origin; configuration is not a public reachability test.
pub fn normalize_connection_url(input:&str)->Result<String>{
    let input=input.trim();
    if input.is_empty()||input.len()>2048||input.chars().any(|c|c.is_whitespace()||c.is_control())||input.contains('\\'){
        bail!("Enter a public HTTPS domain without spaces");
    }
    let candidate=if input.contains("://"){input.to_owned()}else{format!("https://{input}")};
    let (_,authority)=candidate.split_once("://").context("Enter a public HTTPS domain")?;
    if authority.contains('@')||authority.split_once('/').is_some_and(|(_,path)|!path.is_empty()){
        bail!("Enter the domain only, without credentials or a path such as /mcp");
    }
    let url=Url::parse(&candidate).context("Invalid public domain")?;
    if url.scheme()!="https"||!url.username().is_empty()||url.password().is_some()
        ||url.query().is_some()||url.fragment().is_some()||url.path()!="/"||url.port()==Some(0){
        bail!("ChatGPT connection requires an HTTPS origin without credentials, path, query or fragment");
    }
    let host=url.host_str().context("Public domain is missing")?;
    let reserved=["localhost","local","invalid","test","example","example.com","example.net","example.org"];
    if !matches!(url.host(),Some(url::Host::Domain(_)))||!host.contains('.')
        ||host.split('.').any(|label|label.is_empty()||label.len()>63||label.starts_with('-')||label.ends_with('-')||!label.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'))
        ||reserved.iter().any(|name|host==*name||host.ends_with(&format!(".{name}"))){
        bail!("Use your public DNS or ngrok domain, not a local address or example domain");
    }
    Ok(url.origin().ascii_serialization())
}
pub fn valid_id(s: &str) -> bool { !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') }

impl Config {
    /// Fresh installations expose only the local setup page until a public origin is saved.
    pub fn local_setup()->Self {
        let mut config=Self::default();
        config.server.bind="127.0.0.1:20000".parse().unwrap();
        config.server.public_url="http://127.0.0.1:20000".into();
        config.security.allow_http_loopback=true;
        config
    }
    pub fn owner_key_path(&self)->PathBuf{self.security.data_dir.join("owner.key")}
    pub fn public_connection_configured(&self)->bool {
        self.public_url().is_ok_and(|url|url.scheme()=="https")
            && normalize_connection_url(&self.server.public_url).is_ok()
    }
    pub fn load_file(path: &Path) -> Result<Self> {
        let mut c: Self = toml::from_str(&std::fs::read_to_string(path).with_context(|| format!("Read {}; run endlessvibe --init to open local setup, or use --init --workspace NAME=/absolute/root/path", path.display()))?).context("Invalid config.toml")?;
        c.server.public_url = c.server.public_url.trim_end_matches('/').to_owned();c.validate()?;Ok(c)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut c=Self::load_file(path)?;
        if let Ok(value) = env::var("ENDLESSVIBE_BIND") { c.server.bind = value.parse().context("Invalid ENDLESSVIBE_BIND")?; }
        if let Ok(value) = env::var("ENDLESSVIBE_PUBLIC_URL") { c.server.public_url = value; }
        c.server.public_url = c.server.public_url.trim_end_matches('/').to_owned();
        c.validate()?; Ok(c)
    }
    /// Configure encrypted LAN node communication without a public MCP endpoint.
    /// OAuth still needs an internal, loopback-only canonical resource URL.
    pub fn enable_lan_only(&mut self) {
        self.server.lan_only = true;
        self.server.bind = "127.0.0.1:20000".parse().unwrap();
        self.server.public_url = "http://127.0.0.1:20000".into();
        self.security.allow_http_loopback = true;
        self.transfer.enabled = true;
        self.transfer.advertise = true;
        self.transfer.discover = true;
    }
    pub fn public_url(&self) -> Result<Url> { Ok(Url::parse(&self.server.public_url)?) }
    pub fn resource(&self) -> String { format!("{}/mcp", self.server.public_url) }
    pub fn hosts(&self) -> Vec<String> { let mut h = self.server.allowed_hosts.clone(); if let Ok(u) = self.public_url() { if let Some(host) = u.host_str() { h.push(host.to_owned()); } } h.sort(); h.dedup(); h }
    pub fn validate(&self) -> Result<()> {
        if self.server.lan_only {
            if self.server.bind != "127.0.0.1:20000".parse::<SocketAddr>().unwrap()
                || self.server.public_url != "http://127.0.0.1:20000"
                || !self.security.allow_http_loopback || !self.transfer.enabled
                || self.transfer.listen.port() == 20000 {
                bail!("LAN-only mode requires a loopback MCP endpoint, enabled Transfer and the internal local OAuth origin; do not expose MCP on the LAN");
            }
        }
        let url = self.public_url()?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]" | "::1"));
        if url.scheme() != "https" && !(self.security.allow_http_loopback && url.scheme() == "http" && loopback) { bail!("server.public_url must use HTTPS (HTTP loopback is opt-in for local tests)"); }
        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") { bail!("server.public_url must be an origin without credentials, path, query or fragment"); }
        if !self.security.data_dir.is_absolute() { bail!("security.data_dir must be absolute"); }
        if self.security.allow_windows_service_update {
            if !cfg!(windows){bail!("Windows service updates cannot be enabled on other platforms");}
            let project=self.security.windows_update_project.as_ref()
                .context("Windows service update opt-in needs security.windows_update_project")?;
            if !project.is_absolute()||project.components().any(|c|matches!(c,std::path::Component::ParentDir)){
                bail!("Windows self-update Project must be one exact absolute directory without ..");
            }
        }else if self.security.windows_update_project.is_some(){
            bail!("security.windows_update_project requires allow_windows_service_update=true");
        }
        if !(60..=86400).contains(&self.security.access_token_seconds) || !(3600..=90 * 86400).contains(&self.security.refresh_token_seconds) { bail!("Invalid OAuth token lifetime"); }
        if !(1..=16).contains(&self.limits.max_jobs) || !(1..=1000).contains(&self.limits.retained_jobs) || !(1..=3600).contains(&self.limits.command_timeout_seconds) { bail!("Invalid job/timeout limits"); }
        if !(4096..=16 * 1024 * 1024).contains(&self.limits.max_file_bytes) || self.limits.max_read_bytes == 0 || self.limits.max_read_bytes > self.limits.max_file_bytes || !(4096..=8 * 1024 * 1024).contains(&self.limits.max_output_bytes) { bail!("Invalid file/output limits"); }
        if !(1..=100_000).contains(&self.limits.search_max_files) || !(4096..=1024 * 1024 * 1024).contains(&self.limits.search_max_bytes){bail!("search limits must be 1..100000 files and 4096..1073741824 bytes");}
        if !matches!(self.execution.backend.as_str(), "bubblewrap" | "appcontainer" | "host" | "disabled") { bail!("execution.backend must be bubblewrap, appcontainer, host, or disabled"); }
        if self.execution.backend == "host" && !self.execution.acknowledge_unsafe_host_execution { bail!("Host execution requires acknowledge_unsafe_host_execution=true; it has all permissions of the service account"); }
        if !cfg!(target_os="linux")&&self.execution.backend=="bubblewrap"{bail!("bubblewrap is Linux-only; choose host with explicit acknowledgement or disabled");}
        if self.execution.backend=="appcontainer"{if !cfg!(windows){bail!("appcontainer is Windows-only");}if self.execution.allow_network{bail!("appcontainer is offline; set execution.allow_network=false");}}
        let paths=env::split_paths(&self.execution.path).collect::<Vec<_>>();
        if paths.is_empty()||paths.iter().any(|p|p.as_os_str().is_empty()||!p.is_absolute()){bail!("execution.path must contain only absolute PATH entries");}
        let valid_program=|program:&str|!program.is_empty()&&program.len()<=64&&program.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b));
        let mut programs=HashSet::new();for program in &self.execution.allowed_programs{if !valid_program(program)||!programs.insert(program){bail!("execution.allowed_programs must contain unique simple executable names");}}
        let mut required=HashSet::new();for program in &self.execution.required_programs{if !valid_program(program)||!required.insert(program){bail!("execution.required_programs must contain unique simple executable names");}}
        if self.execution.memory_limit_mb < 64 || self.execution.max_processes < 8 { bail!("Execution resource limits too small"); }
        if self.execution.readonly_mounts.len()>64{bail!("execution.readonly_mounts accepts at most 64 entries");}let mut mount_targets=HashSet::new();for m in &self.execution.readonly_mounts { if self.execution.backend=="appcontainer"||(cfg!(windows)&&self.execution.backend!="bubblewrap"&&m.source==m.target){if !m.source.is_absolute()||!m.source.exists()||m.source!=m.target||m.source.components().any(|p|matches!(p,std::path::Component::ParentDir)){bail!("AppContainer read-only roots require one existing absolute native path with source=target and no ..");}}else if !m.source.is_absolute() || !m.source.exists() || !(m.target.starts_with("/opt/") || m.target.starts_with("/cache-readonly/")) || m.target.components().any(|p| matches!(p, std::path::Component::ParentDir)) { bail!("Read-only mounts require an existing absolute source and a /opt/... or /cache-readonly/... destination"); }if !mount_targets.insert(m.target.clone()){bail!("execution.readonly_mounts target paths must be unique");} }
        if self.transfer.listen.port()==0||self.transfer.display_name.trim().is_empty()||self.transfer.display_name.len()>64||self.transfer.display_name.contains(['\n','\r','\0']){bail!("Transfer listener requires a nonzero port and a short safe display name");}
        // Docker is disabled by default. On Windows a Unix socket path cannot
        // be absolute; validate socket paths only when the transport is enabled.
        // Container allowlist validation always applies, even when disabled.
        if self.docker.allowed_containers.len()>64||self.docker.allowed_containers.iter().any(|name|!valid_docker_container(name)){bail!("Docker allowlist supports at most 64 exact, simple container names");}
        let mut docker_names=HashSet::new();if self.docker.allowed_containers.iter().any(|name|!docker_names.insert(name)){bail!("Docker allowed_containers must be unique");}
        if self.docker.enabled {
            #[cfg(windows)]
            bail!("Docker MCP Unix-socket transport is not supported on Windows; set [docker].enabled=false");
            #[cfg(unix)]
            {
                if !self.docker.socket.is_absolute()||self.docker.socket.components().any(|c|matches!(c,std::path::Component::ParentDir)){bail!("Enabled Docker socket must be an absolute path without '..'");}
                if self.docker.allowed_containers.is_empty(){bail!("Docker MCP requires at least one explicitly allowed container name");}
            }
        }
        if !self.git.executable.is_absolute() && !(cfg!(windows)&&self.git.executable==Path::new("git")) || self.git.author_name.trim().is_empty() || self.git.author_email.trim().is_empty() || self.git.author_name.contains(['\n', '\r', '\0']) || self.git.author_email.contains(['\n', '\r', '\0']) { bail!("Invalid Git executable or author identity"); }
        let mut ids = HashSet::new();
        for w in &self.workspaces {
            if !valid_id(&w.id) || !ids.insert(w.id.clone()) || !w.path.is_absolute() { bail!("Workspaces need unique simple IDs and absolute root paths"); }
            if w.projects.is_empty() && (w.allow_exec==Some(true) || w.allow_git_commit==Some(true) || w.allow_git_mutation==Some(true)) && w.allow_write!=Some(true) { bail!("Legacy workspace {}: execution/commit/git mutation also require allow_write", w.id); }
            if w.projects.is_empty() && w.allow_git_mutation==Some(true) && w.allow_exec!=Some(true) { bail!("Legacy workspace {}: allow_git_mutation requires allow_exec=true", w.id); }
            if w.projects.is_empty(){if let Some(profile)=&w.execution_profile{if !matches!(profile.as_str(),"isolated"|"development"|"trusted_host"){bail!("Legacy workspace {}: execution_profile must be isolated, development or trusted_host",w.id);}if profile=="trusted_host" && (self.execution.backend!="host"||!self.execution.acknowledge_unsafe_host_execution||w.allow_exec!=Some(true)||w.allow_write!=Some(true)){bail!("Legacy workspace {}: trusted_host requires acknowledged host backend and write + exec permissions",w.id);}}parse_environment_entries(&w.environment).with_context(||format!("Legacy workspace {} environment is invalid",w.id))?;}
            let mut projects=HashSet::new();
            for p in &w.projects {
                if !valid_id(&p.id)||!projects.insert(p.id.clone())||p.path.is_absolute()||p.path.components().any(|c|matches!(c,std::path::Component::ParentDir|std::path::Component::RootDir|std::path::Component::Prefix(_))){bail!("Workspace {} projects need unique simple IDs and relative paths without '..'",w.id);}
                if (p.allow_exec||p.allow_git_commit)&&!p.allow_write{bail!("Project {}/{}: execution/commit also require allow_write",w.id,p.id);}
                if p.allow_git_mutation&&(!p.allow_write||!p.allow_exec){bail!("Project {}/{}: allow_git_mutation requires allow_write=true and allow_exec=true",w.id,p.id);}
                if !matches!(p.execution_profile.as_str(),"isolated"|"development"|"trusted_host"){bail!("Project {}/{}: execution_profile must be isolated, development or trusted_host",w.id,p.id);}if p.trusted_host()&&(self.execution.backend!="host"||!self.execution.acknowledge_unsafe_host_execution||!p.allow_write||!p.allow_exec){bail!("Project {}/{}: trusted_host requires acknowledged host backend and write + exec permissions",w.id,p.id);}
                p.environment_map().with_context(||format!("Project {}/{} environment is invalid",w.id,p.id))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test]
    fn inherited_path_filters_relative_empty_and_repeated_entries(){
        let valid=env::current_dir().unwrap();
        let extra=valid.join("toolchain-bin");
        let inherited=env::join_paths([
            valid.as_path(),Path::new("relative/bin"),Path::new("."),
            Path::new(".."),Path::new(""),extra.as_path(),valid.as_path()
        ]).unwrap();
        let selected=inherited_absolute_path_entries(&inherited);
        assert_eq!(selected,vec![valid.clone(),extra.clone()]);
        let value=env::join_paths(&selected).unwrap().into_string().unwrap();
        let mut config=Config::default();
        config.execution.path=value;
        config.validate().unwrap();
        config.execution.path=env::join_paths([Path::new("relative/bin")])
            .unwrap().into_string().unwrap();
        assert!(config.validate().unwrap_err().to_string().contains("absolute PATH"),
            "Manual/explicit execution.path entries remain strictly validated");
    }
    #[cfg(windows)]
    #[test]
    fn windows_first_run_path_accepts_malformed_inherited_environment(){
        use std::ffi::OsStr;
        let raw=OsStr::new(
            r"C:\tools\bin;.;C:relative;%SystemRoot%\System32;;D:\Rust\bin;C:\tools\bin;..\bin"
        );
        let sanitized=windows_default_execution_path(
            Some(raw),Some(OsStr::new(r"D:\Windows")));
        let entries=env::split_paths(&sanitized).collect::<Vec<_>>();
        assert_eq!(entries[0],PathBuf::from(r"D:\Windows\System32"));
        assert_eq!(entries[1],PathBuf::from(r"D:\Windows"));
        assert!(entries.contains(&PathBuf::from(r"C:\tools\bin")));
        assert!(entries.contains(&PathBuf::from(r"D:\Rust\bin")));
        assert_eq!(entries.iter().filter(|p|**p==PathBuf::from(r"C:\tools\bin")).count(),1);
        assert!(entries.iter().all(|p|p.is_absolute()
            &&!p.components().any(|c|matches!(c,std::path::Component::ParentDir))));
        assert!(!sanitized.contains("%SystemRoot%"));
        let mut config=Config::default();
        config.execution.path=sanitized;
        config.validate().unwrap();
        let fallback=windows_default_execution_path(None,None);
        assert_eq!(fallback,r"C:\Windows\System32;C:\Windows");
        let no_root=windows_default_execution_path(Some(OsStr::new("relative;.;")),None);
        assert_eq!(no_root,fallback);
    }

    #[test]
    fn windows_service_update_requires_explicit_exact_project_opt_in(){
        let mut config=Config::default();
        assert!(!config.security.allow_windows_service_update);
        assert!(config.security.windows_update_project.is_none());
        assert!(config.validate().is_ok());
        config.security.allow_windows_service_update=true;
        assert!(config.validate().is_err(),"Updating without a configured Project must fail");
        config.security.windows_update_project=Some(PathBuf::from("relative/project"));
        assert!(config.validate().is_err(),"Relative self-update project paths must fail");
        config.security.windows_update_project=Some(if cfg!(windows){
            PathBuf::from(r"F:\project\EndlessVibe")
        }else{PathBuf::from("/tmp/EndlessVibe")});
        if cfg!(windows){assert!(config.validate().is_ok());}
        else{assert!(config.validate().is_err(),"Non-Windows platforms cannot opt in");}
        config.security.allow_windows_service_update=false;
        assert!(config.validate().is_err(),"An opt-in path alone must not enable updates");
    }
    #[test]fn search_resource_limits_are_bounded(){let mut c=Config::default();assert!(c.validate().is_ok());c.limits.search_max_files=0;assert!(c.validate().is_err());c.limits.search_max_files=100001;assert!(c.validate().is_err());c.limits.search_max_files=5000;c.limits.search_max_bytes=1024*1024*1024+1;assert!(c.validate().is_err());}
    #[test]
    fn lan_only_uses_loopback_mcp_and_encrypted_transfer_nodes() {
        let mut c = Config::default();
        c.enable_lan_only();
        assert!(c.server.lan_only);
        assert_eq!(c.server.bind.to_string(), "127.0.0.1:20000");
        assert_eq!(c.server.public_url, "http://127.0.0.1:20000");
        assert!(c.security.allow_http_loopback);
        assert!(c.transfer.enabled && c.transfer.advertise && c.transfer.discover);
        assert_eq!(c.transfer.listen.port(), 20002);
        c.validate().unwrap();
        let encoded = toml::to_string_pretty(&c).unwrap();
        let decoded: Config = toml::from_str(&encoded).unwrap();
        assert!(decoded.server.lan_only);
        decoded.validate().unwrap();

        let mut bad = c.clone();
        bad.server.bind = "0.0.0.0:20000".parse().unwrap();
        assert!(bad.validate().is_err(), "MCP must never bind a LAN interface");
        bad = c.clone();
        bad.server.public_url = "http://192.168.1.12:20000".into();
        assert!(bad.validate().is_err(), "LAN HTTP must not carry OAuth tokens");
        bad = c.clone();
        bad.transfer.enabled = false;
        assert!(bad.validate().is_err(), "LAN mode requires TLS Transfer");
        bad = c.clone();
        bad.security.allow_http_loopback = false;
        assert!(bad.validate().is_err(), "Internal OAuth origin must be valid");
        bad = c.clone();
        bad.transfer.listen = "0.0.0.0:20000".parse().unwrap();
        assert!(bad.validate().is_err(), "Transfer must not reuse the MCP port");
    }
    #[test]
    fn existing_configs_default_to_normal_mode() {
        let c = Config::default();
        assert!(!c.server.lan_only);
        let mut doc: toml::Value = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        doc["server"].as_table_mut().unwrap().remove("lan_only");
        let old: Config = doc.try_into().unwrap();
        assert!(!old.server.lan_only);
        old.validate().unwrap();
    }
    #[test]fn transfer_is_disabled_by_default(){let mut c=Config::default();assert!(!c.transfer.enabled);assert_eq!(c.transfer.listen.port(),20002);c.transfer.display_name.clear();assert!(c.validate().is_err());}
    #[test]fn docker_requires_exact_allowlist_and_is_disabled_by_default(){
        let mut c=Config::default();
        assert!(!c.docker.enabled);
        assert!(!c.docker.allow_project_socket);
        c.docker.enabled=true;
        assert!(c.validate().is_err());
        c.docker.allowed_containers=vec!["endlessvibe".into()];
        #[cfg(unix)]
        assert!(c.validate().is_ok());
        #[cfg(windows)]
        assert!(c.validate().unwrap_err().to_string().contains("Windows"));
        c.docker.allowed_containers.push("../other".into());
        assert!(c.validate().is_err());
        c.docker.allowed_containers.pop();
        c.docker.allowed_containers.push("endlessvibe".into());
        assert!(c.validate().is_err());
    }
    #[test]fn disabled_docker_does_not_require_a_unix_socket(){
        let mut c=Config::default();
        assert!(!c.docker.enabled);
        c.docker.socket=PathBuf::new();
        assert!(c.validate().is_ok());
        // An inactive socket setting must not prevent Windows initialization.
        c.docker.socket="../not-a-docker.sock".into();
        assert!(c.validate().is_ok());
        // Inactive does not mean that invalid allowlists become acceptable.
        c.docker.allowed_containers=vec!["../unsafe".into()];
        assert!(c.validate().is_err());
    }
    #[cfg(unix)]
    #[test]fn enabled_docker_rejects_invalid_unix_socket_paths(){
        let mut c=Config::default();
        c.docker.enabled=true;
        c.docker.allowed_containers=vec!["allowed".into()];
        c.docker.socket=PathBuf::new();
        assert!(c.validate().is_err());
        c.docker.socket="../relative.sock".into();
        assert!(c.validate().is_err());
        c.docker.socket="/tmp/../docker.sock".into();
        assert!(c.validate().is_err());
        c.docker.socket="/var/run/docker.sock".into();
        assert!(c.validate().is_ok());
    }
    #[cfg(windows)]
    #[test]fn windows_default_config_serializes_and_validates_for_init(){
        let root=tempfile::tempdir().unwrap();
        let mut c=Config::default();
        assert!(c.docker.socket.as_os_str().is_empty());
        c.workspaces.push(WorkspaceConfig{
            id:"project".into(),path:root.path().to_owned(),projects:vec![],
            allow_write:None,allow_exec:None,allow_git_commit:None,
            allow_git_mutation:None,allow_git_push:None,execution_profile:None,
            environment:vec![]
        });
        c.validate().expect("default Windows settings should permit --init");
        let text=toml::to_string_pretty(&c).unwrap();
        let decoded:Config=toml::from_str(&text).unwrap();
        decoded.validate().expect("generated Windows config should be loadable");
        assert!(decoded.docker.socket.as_os_str().is_empty());
        c.docker.enabled=true;
        c.docker.allowed_containers.push("allowed".into());
        assert!(c.validate().unwrap_err().to_string().contains("Windows"));
    }
    #[test]
    fn local_setup_preserves_platform_safety_without_authorizing_projects(){
        let c=Config::local_setup();c.validate().unwrap();
        assert!(c.server.bind.ip().is_loopback());assert_eq!(c.server.bind.port(),20000);
        assert_eq!(c.server.public_url,"http://127.0.0.1:20000");
        assert!(c.security.allow_http_loopback);assert!(!c.server.lan_only);
        assert!(!c.transfer.enabled);assert!(c.workspaces.is_empty());
        assert_eq!(c.execution.backend,Config::default().execution.backend);
        assert!(!c.execution.allow_shell&&!c.execution.allow_network&&!c.execution.acknowledge_unsafe_host_execution);
        assert!(!c.public_connection_configured());assert!(!Config::default().public_connection_configured());
    }
    #[test]
    fn connection_url_normalizes_domains_and_rejects_misleading_origins(){
        assert_eq!(normalize_connection_url("  Demo.ngrok-free.app  ").unwrap(),"https://demo.ngrok-free.app");
        assert_eq!(normalize_connection_url("https://demo.ngrok.app:443/").unwrap(),"https://demo.ngrok.app");
        assert_eq!(normalize_connection_url("demo.ngrok.app:8443").unwrap(),"https://demo.ngrok.app:8443");
        for bad in ["","http://demo.ngrok.app","https://localhost","https://127.0.0.1","https://[::1]",
            "mcp.example.com","demo.local","demo.invalid","demo.test","https://user:pass@demo.ngrok.app",
            "https://@demo.ngrok.app","demo.ngrok.app/mcp","demo.ngrok.app/mcp/..","demo.ngrok.app/%2e",
            "demo.ngrok.app?token=secret","demo.ngrok.app#fragment","demo.ngrok.app:0","//demo.ngrok.app",
            "demo.ngrok.app\\mcp","demo.ngrok.app\n.evil","demo..ngrok.app","-demo.ngrok.app"]{
            assert!(normalize_connection_url(bad).is_err(),"Accepted {bad:?}");
        }
    }
    #[test] fn defaults_are_protected() { let c = Config::default(); assert!(c.validate().is_ok()); assert_eq!(c.execution.backend, if cfg!(target_os="linux"){"bubblewrap"}else{"disabled"}); assert!(!c.execution.allow_network); assert!(!c.execution.allow_shell); }
    #[test]
    fn appcontainer_requires_windows_and_offline_network(){
        let mut c=Config::default();c.execution.backend="appcontainer".into();
        assert_eq!(c.validate().is_ok(),cfg!(windows));
        c.execution.allow_network=true;assert!(c.validate().is_err());
    }
    #[cfg(windows)]
    #[test]
    fn appcontainer_readonly_roots_are_existing_native_paths(){
        let root=tempfile::tempdir().unwrap();let mut c=Config::default();
        c.execution.backend="appcontainer".into();
        c.execution.readonly_mounts=vec![ReadOnlyMount{source:root.path().to_owned(),target:root.path().to_owned()}];
        assert!(c.validate().is_ok());
        c.execution.readonly_mounts[0].target=root.path().join("other");
        assert!(c.validate().is_err());
        c.execution.readonly_mounts[0].source=root.path().join("missing");
        c.execution.readonly_mounts[0].target=root.path().join("missing");
        assert!(c.validate().is_err());
    }
    #[cfg(windows)]
    #[test]
    fn native_readonly_roots_can_be_preserved_when_disabling_appcontainer(){
        let root=tempfile::tempdir().unwrap();let mut c=Config::default();
        c.execution.readonly_mounts=vec![ReadOnlyMount{source:root.path().to_owned(),target:root.path().to_owned()}];
        for backend in ["appcontainer","disabled","host"]{
            c.execution.backend=backend.into();c.execution.acknowledge_unsafe_host_execution=backend=="host";
            c.validate().expect("Switching away from the sandbox must preserve native read-only roots");
        }
        c.execution.backend="appcontainer".into();c.execution.acknowledge_unsafe_host_execution=false;
        c.execution.readonly_mounts[0].target="/opt/tool".into();
        assert!(c.validate().is_err());
    }
    #[test] fn host_mode_needs_acknowledgement() { let mut c = Config::default(); c.execution.backend = "host".into(); assert!(c.validate().is_err()); }
    #[test] fn http_public_server_is_rejected() { let mut c = Config::default(); c.server.public_url = "http://example.com".into(); assert!(c.validate().is_err()); }
    #[test] fn ids_are_bounded() { assert!(valid_id("DemoApp")); assert!(!valid_id("../DemoApp")); assert!(!valid_id("")); }
    #[test] fn projects_are_relative_to_workspace_roots(){let root=tempfile::tempdir().unwrap();let mut c=Config::default();c.workspaces=vec![WorkspaceConfig{id:"root".into(),path:root.path().to_owned(),projects:vec![ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:default_project_profile(),environment:vec![]}],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];assert!(c.validate().is_ok());c.workspaces[0].projects[0].path=root.path().join("app");assert!(c.validate().is_err());}
    #[test] fn git_mutation_requires_write_and_exec(){let root=tempfile::tempdir().unwrap();let mut c=Config::default();c.workspaces=vec![WorkspaceConfig{id:"root".into(),path:root.path().to_owned(),projects:vec![ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:false,allow_git_commit:false,allow_git_mutation:true,allow_git_push:false,execution_profile:default_project_profile(),environment:vec![]}],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];assert!(c.validate().is_err());c.workspaces[0].projects[0].allow_exec=true;assert!(c.validate().is_ok());}
    #[test] fn allowed_programs_are_simple_and_unique(){let mut c=Config::default();c.execution.allowed_programs=vec!["cargo".into(),"cargo".into()];assert!(c.validate().is_err());c.execution.allowed_programs=vec!["/usr/bin/cargo".into()];assert!(c.validate().is_err());c.execution.allowed_programs=vec!["cargo".into(),"rustc".into()];assert!(c.validate().is_ok());}
    #[test] fn required_programs_are_simple_and_may_overlap_allowed(){let mut c=Config::default();c.execution.required_programs=vec!["cargo".into(),"postgres".into()];assert!(c.validate().is_ok());c.execution.required_programs=vec!["postgres".into(),"postgres".into()];assert!(c.validate().is_err());c.execution.required_programs=vec!["/opt/postgres/bin/postgres".into()];assert!(c.validate().is_err());}
    #[test]
    fn trusted_host_profile_requires_explicit_host_acknowledgement(){
      let root=tempfile::tempdir().unwrap();
      let mut c=Config::default();
      c.workspaces=vec![WorkspaceConfig{id:"root".into(),path:root.path().to_owned(),projects:vec![
        ProjectConfig{id:"kernel".into(),path:"kernel".into(),allow_write:true,allow_exec:true,
          allow_git_commit:false,allow_git_mutation:false,allow_git_push:false,
          execution_profile:"trusted_host".into(),environment:vec![]}
      ],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,
        allow_git_push:None,execution_profile:None,environment:vec![]}];
      assert!(c.validate().is_err());
      c.execution.backend="host".into();
      assert!(c.validate().is_err());
      c.execution.acknowledge_unsafe_host_execution=true;
      assert!(c.validate().is_ok());
      c.workspaces[0].projects[0].allow_exec=false;
      assert!(c.validate().is_err());
      c.workspaces[0].projects[0].allow_exec=true;
      c.workspaces[0].projects[0].execution_profile="development".into();
      assert!(c.validate().is_ok());
      c.execution.backend="bubblewrap".into();
      assert_eq!(c.validate().is_ok(),cfg!(target_os="linux"));
    }
    #[test]fn project_environment_and_profiles_are_validated(){assert!(parse_environment_entries(&["TEST_DATABASE_URL=postgres://localhost/test".into()]).is_ok());assert!(parse_environment_entries(&["PATH=/tmp".into()]).is_err());let p=ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:"development".into(),environment:vec!["A=1".into()]};assert!(p.development());assert_eq!(p.environment_map().unwrap()["A"],"1");}
}
