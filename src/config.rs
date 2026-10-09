use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap,HashSet}, env, net::SocketAddr, path::{Path, PathBuf}};
use url::Url;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config { pub server: Server, pub security: Security, pub limits: Limits, pub execution: Execution, pub git: Git, pub docker: Docker, pub transfer:Transfer, pub workspaces: Vec<WorkspaceConfig> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server { pub bind: SocketAddr, pub public_url: String, pub allowed_hosts: Vec<String> }
impl Default for Server { fn default() -> Self { Self { bind: "0.0.0.0:20000".parse().unwrap(), public_url: "https://endlessvibe.soymilk.xin".into(), allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "host.docker.internal".into(), "endlessvibe".into()] } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Security { pub data_dir: PathBuf, pub allow_http_loopback: bool, pub extra_redirect_uris: Vec<String>, pub access_token_seconds: u64, pub refresh_token_seconds: u64 }
impl Default for Security { fn default() -> Self { Self { data_dir: default_state_dir(), allow_http_loopback: false, extra_redirect_uris: vec![], access_token_seconds: 3600, refresh_token_seconds: 30 * 86400 } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits { pub max_file_bytes: usize, pub max_read_bytes: usize, pub max_output_bytes: usize, pub command_timeout_seconds: u64, pub max_jobs: usize, pub retained_jobs: usize, pub search_max_files: usize, pub search_max_bytes: usize }
impl Default for Limits { fn default() -> Self { Self { max_file_bytes: 2 * 1024 * 1024, max_read_bytes: 256 * 1024, max_output_bytes: 1024 * 1024, command_timeout_seconds: 120, max_jobs: 2, retained_jobs: 100, search_max_files: 5000, search_max_bytes: 64 * 1024 * 1024 } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Execution { pub backend: String, pub acknowledge_unsafe_host_execution: bool, pub allow_shell: bool, pub allow_network: bool, pub auto_discover_toolchains: bool, pub bubblewrap: PathBuf, pub path: String, pub allowed_programs: Vec<String>, pub required_programs: Vec<String>, pub readonly_mounts: Vec<ReadOnlyMount>, pub memory_limit_mb: u64, pub max_processes: u64 }
impl Default for Execution { fn default() -> Self { Self { backend: "bubblewrap".into(), acknowledge_unsafe_host_execution: false, allow_shell: false, allow_network: false, auto_discover_toolchains: true, bubblewrap: "/usr/bin/bwrap".into(), path: "/usr/local/bin:/usr/bin:/bin".into(), allowed_programs: ["cargo", "rustc", "cmake", "ninja", "make", "ctest", "python3", "git", "rg"].map(str::to_owned).to_vec(), required_programs: vec![], readonly_mounts: vec![], memory_limit_mb: 8192, max_processes: 256 } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyMount { pub source: PathBuf, pub target: PathBuf }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Git { pub executable: PathBuf, pub author_name: String, pub author_email: String }
impl Default for Git { fn default() -> Self { Self { executable: "/usr/bin/git".into(), author_name: "EndlessVibe".into(), author_email: "endlessvibe@localhost".into() } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Docker { pub enabled:bool, pub socket:PathBuf, pub allowed_containers:Vec<String>, pub allow_start:bool, pub allow_stop:bool, pub allow_restart:bool, pub allow_project_socket:bool }
impl Default for Docker { fn default()->Self{Self{enabled:false,socket:"/var/run/docker.sock".into(),allowed_containers:vec![],allow_start:false,allow_stop:false,allow_restart:false,allow_project_socket:false}} }
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
impl ProjectConfig{pub fn development(&self)->bool{self.execution_profile=="development"}pub fn environment_map(&self)->Result<BTreeMap<String,String>>{parse_environment_entries(&self.environment)}}

pub fn default_config_path() -> PathBuf { env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("endlessvibe/config.toml") }
pub fn default_state_dir() -> PathBuf { env::var_os("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/state")).join("endlessvibe") }
fn home() -> PathBuf { env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")) }
pub fn valid_id(s: &str) -> bool { !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') }

impl Config {
    pub fn load_file(path: &Path) -> Result<Self> {
        let mut c: Self = toml::from_str(&std::fs::read_to_string(path).with_context(|| format!("Read {}; initialize first with --init --workspace NAME=/absolute/root/path", path.display()))?).context("Invalid config.toml")?;
        c.server.public_url = c.server.public_url.trim_end_matches('/').to_owned();c.validate()?;Ok(c)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut c=Self::load_file(path)?;
        if let Ok(value) = env::var("ENDLESSVIBE_BIND") { c.server.bind = value.parse().context("Invalid ENDLESSVIBE_BIND")?; }
        if let Ok(value) = env::var("ENDLESSVIBE_PUBLIC_URL") { c.server.public_url = value; }
        c.server.public_url = c.server.public_url.trim_end_matches('/').to_owned();
        c.validate()?; Ok(c)
    }
    pub fn public_url(&self) -> Result<Url> { Ok(Url::parse(&self.server.public_url)?) }
    pub fn resource(&self) -> String { format!("{}/mcp", self.server.public_url) }
    pub fn hosts(&self) -> Vec<String> { let mut h = self.server.allowed_hosts.clone(); if let Ok(u) = self.public_url() { if let Some(host) = u.host_str() { h.push(host.to_owned()); } } h.sort(); h.dedup(); h }
    pub fn validate(&self) -> Result<()> {
        let url = self.public_url()?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]" | "::1"));
        if url.scheme() != "https" && !(self.security.allow_http_loopback && url.scheme() == "http" && loopback) { bail!("server.public_url must use HTTPS (HTTP loopback is opt-in for local tests)"); }
        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || !matches!(url.path(), "" | "/") { bail!("server.public_url must be an origin without credentials, path, query or fragment"); }
        if !self.security.data_dir.is_absolute() { bail!("security.data_dir must be absolute"); }
        if !(60..=86400).contains(&self.security.access_token_seconds) || !(3600..=90 * 86400).contains(&self.security.refresh_token_seconds) { bail!("Invalid OAuth token lifetime"); }
        if !(1..=16).contains(&self.limits.max_jobs) || !(1..=1000).contains(&self.limits.retained_jobs) || !(1..=3600).contains(&self.limits.command_timeout_seconds) { bail!("Invalid job/timeout limits"); }
        if !(4096..=16 * 1024 * 1024).contains(&self.limits.max_file_bytes) || self.limits.max_read_bytes == 0 || self.limits.max_read_bytes > self.limits.max_file_bytes || !(4096..=8 * 1024 * 1024).contains(&self.limits.max_output_bytes) { bail!("Invalid file/output limits"); }
        if !(1..=100_000).contains(&self.limits.search_max_files) || !(4096..=1024 * 1024 * 1024).contains(&self.limits.search_max_bytes){bail!("search limits must be 1..100000 files and 4096..1073741824 bytes");}
        if !matches!(self.execution.backend.as_str(), "bubblewrap" | "host" | "disabled") { bail!("execution.backend must be bubblewrap, host, or disabled"); }
        if self.execution.backend == "host" && !self.execution.acknowledge_unsafe_host_execution { bail!("Host execution requires acknowledge_unsafe_host_execution=true; it has all permissions of the service account"); }
        if self.execution.path.split(':').any(|p| p.is_empty() || !Path::new(p).is_absolute()) { bail!("execution.path must contain only absolute PATH entries"); }
        let valid_program=|program:&str|!program.is_empty()&&program.len()<=64&&program.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b));
        let mut programs=HashSet::new();for program in &self.execution.allowed_programs{if !valid_program(program)||!programs.insert(program){bail!("execution.allowed_programs must contain unique simple executable names");}}
        let mut required=HashSet::new();for program in &self.execution.required_programs{if !valid_program(program)||!required.insert(program){bail!("execution.required_programs must contain unique simple executable names");}}
        if self.execution.memory_limit_mb < 64 || self.execution.max_processes < 8 { bail!("Execution resource limits too small"); }
        if self.execution.readonly_mounts.len()>64{bail!("execution.readonly_mounts accepts at most 64 entries");}let mut mount_targets=HashSet::new();for m in &self.execution.readonly_mounts { if !m.source.is_absolute() || !m.source.exists() || !(m.target.starts_with("/opt/") || m.target.starts_with("/cache-readonly/")) || m.target.components().any(|p| matches!(p, std::path::Component::ParentDir)) { bail!("Read-only mounts require an existing absolute source and a /opt/... or /cache-readonly/... destination"); }if !mount_targets.insert(m.target.clone()){bail!("execution.readonly_mounts target paths must be unique");} }
        if self.transfer.listen.port()==0||self.transfer.display_name.trim().is_empty()||self.transfer.display_name.len()>64||self.transfer.display_name.contains(['\n','\r','\0']){bail!("Transfer listener requires a nonzero port and a short safe display name");}
        if !self.docker.socket.is_absolute()||self.docker.socket.components().any(|c|matches!(c,std::path::Component::ParentDir))||self.docker.allowed_containers.len()>64||self.docker.allowed_containers.iter().any(|name|!valid_docker_container(name)) { bail!("Docker socket must be an absolute path without '..'; allowlist supports at most 64 exact, simple container names"); }
        let mut docker_names=HashSet::new();if self.docker.allowed_containers.iter().any(|name|!docker_names.insert(name)){bail!("Docker allowed_containers must be unique");}
        if self.docker.enabled&&self.docker.allowed_containers.is_empty(){bail!("Docker MCP requires at least one explicitly allowed container name");}
        if !self.git.executable.is_absolute() || self.git.author_name.trim().is_empty() || self.git.author_email.trim().is_empty() || self.git.author_name.contains(['\n', '\r', '\0']) || self.git.author_email.contains(['\n', '\r', '\0']) { bail!("Invalid Git executable or author identity"); }
        let mut ids = HashSet::new();
        for w in &self.workspaces {
            if !valid_id(&w.id) || !ids.insert(w.id.clone()) || !w.path.is_absolute() { bail!("Workspaces need unique simple IDs and absolute root paths"); }
            if w.projects.is_empty() && (w.allow_exec==Some(true) || w.allow_git_commit==Some(true) || w.allow_git_mutation==Some(true)) && w.allow_write!=Some(true) { bail!("Legacy workspace {}: execution/commit/git mutation also require allow_write", w.id); }
            if w.projects.is_empty() && w.allow_git_mutation==Some(true) && w.allow_exec!=Some(true) { bail!("Legacy workspace {}: allow_git_mutation requires allow_exec=true", w.id); }
            if w.projects.is_empty(){if let Some(profile)=&w.execution_profile{if !matches!(profile.as_str(),"isolated"|"development"){bail!("Legacy workspace {}: execution_profile must be isolated or development",w.id);}}parse_environment_entries(&w.environment).with_context(||format!("Legacy workspace {} environment is invalid",w.id))?;}
            let mut projects=HashSet::new();
            for p in &w.projects {
                if !valid_id(&p.id)||!projects.insert(p.id.clone())||p.path.is_absolute()||p.path.components().any(|c|matches!(c,std::path::Component::ParentDir|std::path::Component::RootDir|std::path::Component::Prefix(_))){bail!("Workspace {} projects need unique simple IDs and relative paths without '..'",w.id);}
                if (p.allow_exec||p.allow_git_commit)&&!p.allow_write{bail!("Project {}/{}: execution/commit also require allow_write",w.id,p.id);}
                if p.allow_git_mutation&&(!p.allow_write||!p.allow_exec){bail!("Project {}/{}: allow_git_mutation requires allow_write=true and allow_exec=true",w.id,p.id);}
                if !matches!(p.execution_profile.as_str(),"isolated"|"development"){bail!("Project {}/{}: execution_profile must be isolated or development",w.id,p.id);}
                p.environment_map().with_context(||format!("Project {}/{} environment is invalid",w.id,p.id))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test]fn search_resource_limits_are_bounded(){let mut c=Config::default();assert!(c.validate().is_ok());c.limits.search_max_files=0;assert!(c.validate().is_err());c.limits.search_max_files=100001;assert!(c.validate().is_err());c.limits.search_max_files=5000;c.limits.search_max_bytes=1024*1024*1024+1;assert!(c.validate().is_err());}
    #[test]fn transfer_is_disabled_by_default(){let mut c=Config::default();assert!(!c.transfer.enabled);assert_eq!(c.transfer.listen.port(),20002);c.transfer.display_name.clear();assert!(c.validate().is_err());}
    #[test]fn docker_requires_exact_allowlist_and_is_disabled_by_default(){let mut c=Config::default();assert!(!c.docker.enabled);assert!(!c.docker.allow_project_socket);c.docker.enabled=true;assert!(c.validate().is_err());c.docker.allowed_containers=vec!["endlessvibe".into()];assert!(c.validate().is_ok());c.docker.allowed_containers.push("../other".into());assert!(c.validate().is_err());c.docker.allowed_containers.pop();c.docker.allowed_containers.push("endlessvibe".into());assert!(c.validate().is_err());}
    #[test] fn defaults_are_protected() { let c = Config::default(); assert!(c.validate().is_ok()); assert_eq!(c.execution.backend, "bubblewrap"); assert!(!c.execution.allow_network); assert!(!c.execution.allow_shell); }
    #[test] fn host_mode_needs_acknowledgement() { let mut c = Config::default(); c.execution.backend = "host".into(); assert!(c.validate().is_err()); }
    #[test] fn http_public_server_is_rejected() { let mut c = Config::default(); c.server.public_url = "http://example.com".into(); assert!(c.validate().is_err()); }
    #[test] fn ids_are_bounded() { assert!(valid_id("BAfter")); assert!(!valid_id("../BAfter")); assert!(!valid_id("")); }
    #[test] fn projects_are_relative_to_workspace_roots(){let mut c=Config::default();c.workspaces=vec![WorkspaceConfig{id:"root".into(),path:"/tmp/root".into(),projects:vec![ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:default_project_profile(),environment:vec![]}],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];assert!(c.validate().is_ok());c.workspaces[0].projects[0].path="/tmp/root/app".into();assert!(c.validate().is_err());}
    #[test] fn git_mutation_requires_write_and_exec(){let mut c=Config::default();c.workspaces=vec![WorkspaceConfig{id:"root".into(),path:"/tmp/root".into(),projects:vec![ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:false,allow_git_commit:false,allow_git_mutation:true,allow_git_push:false,execution_profile:default_project_profile(),environment:vec![]}],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];assert!(c.validate().is_err());c.workspaces[0].projects[0].allow_exec=true;assert!(c.validate().is_ok());}
    #[test] fn allowed_programs_are_simple_and_unique(){let mut c=Config::default();c.execution.allowed_programs=vec!["cargo".into(),"cargo".into()];assert!(c.validate().is_err());c.execution.allowed_programs=vec!["/usr/bin/cargo".into()];assert!(c.validate().is_err());c.execution.allowed_programs=vec!["cargo".into(),"rustc".into()];assert!(c.validate().is_ok());}
    #[test] fn required_programs_are_simple_and_may_overlap_allowed(){let mut c=Config::default();c.execution.required_programs=vec!["cargo".into(),"postgres".into()];assert!(c.validate().is_ok());c.execution.required_programs=vec!["postgres".into(),"postgres".into()];assert!(c.validate().is_err());c.execution.required_programs=vec!["/opt/postgres/bin/postgres".into()];assert!(c.validate().is_err());}
    #[test]fn project_environment_and_profiles_are_validated(){assert!(parse_environment_entries(&["TEST_DATABASE_URL=postgres://localhost/test".into()]).is_ok());assert!(parse_environment_entries(&["PATH=/tmp".into()]).is_err());let p=ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:"development".into(),environment:vec!["A=1".into()]};assert!(p.development());assert_eq!(p.environment_map().unwrap()["A"],"1");}
}
