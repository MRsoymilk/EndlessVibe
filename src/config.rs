use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, env, net::SocketAddr, path::{Path, PathBuf}};
use url::Url;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config { pub server: Server, pub security: Security, pub limits: Limits, pub execution: Execution, pub git: Git, pub workspaces: Vec<WorkspaceConfig> }
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
pub struct Execution { pub backend: String, pub acknowledge_unsafe_host_execution: bool, pub allow_shell: bool, pub allow_network: bool, pub bubblewrap: PathBuf, pub path: String, pub allowed_programs: Vec<String>, pub readonly_mounts: Vec<ReadOnlyMount>, pub memory_limit_mb: u64, pub max_processes: u64 }
impl Default for Execution { fn default() -> Self { Self { backend: "bubblewrap".into(), acknowledge_unsafe_host_execution: false, allow_shell: false, allow_network: false, bubblewrap: "/usr/bin/bwrap".into(), path: "/usr/local/bin:/usr/bin:/bin".into(), allowed_programs: ["cargo", "rustc", "cmake", "ninja", "make", "ctest", "python3", "git", "rg"].map(str::to_owned).to_vec(), readonly_mounts: vec![], memory_limit_mb: 8192, max_processes: 256 } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadOnlyMount { pub source: PathBuf, pub target: PathBuf }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Git { pub executable: PathBuf, pub author_name: String, pub author_email: String }
impl Default for Git { fn default() -> Self { Self { executable: "/usr/bin/git".into(), author_name: "EndlessVibe".into(), author_email: "endlessvibe@localhost".into() } } }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig { pub id: String, pub path: PathBuf, #[serde(default, skip_serializing_if = "Vec::is_empty")] pub projects: Vec<ProjectConfig>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_write: Option<bool>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_exec: Option<bool>, #[serde(default,skip_serializing_if="Option::is_none")] pub allow_git_commit: Option<bool> }
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig { pub id: String, pub path: PathBuf, #[serde(default)] pub allow_write: bool, #[serde(default)] pub allow_exec: bool, #[serde(default)] pub allow_git_commit: bool }

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
        if !matches!(self.execution.backend.as_str(), "bubblewrap" | "host" | "disabled") { bail!("execution.backend must be bubblewrap, host, or disabled"); }
        if self.execution.backend == "host" && !self.execution.acknowledge_unsafe_host_execution { bail!("Host execution requires acknowledge_unsafe_host_execution=true; it has all permissions of the service account"); }
        if self.execution.path.split(':').any(|p| p.is_empty() || !Path::new(p).is_absolute()) { bail!("execution.path must contain only absolute PATH entries"); }
        if self.execution.memory_limit_mb < 64 || self.execution.max_processes < 8 { bail!("Execution resource limits too small"); }
        for m in &self.execution.readonly_mounts { if !m.source.is_absolute() || !m.source.exists() || !(m.target.starts_with("/opt/") || m.target.starts_with("/cache-readonly/")) || m.target.components().any(|p| matches!(p, std::path::Component::ParentDir)) { bail!("Read-only mounts require an existing absolute source and a /opt/... or /cache-readonly/... destination"); } }
        if !self.git.executable.is_absolute() || self.git.author_name.trim().is_empty() || self.git.author_email.trim().is_empty() || self.git.author_name.contains(['\n', '\r', '\0']) || self.git.author_email.contains(['\n', '\r', '\0']) { bail!("Invalid Git executable or author identity"); }
        let mut ids = HashSet::new();
        for w in &self.workspaces {
            if !valid_id(&w.id) || !ids.insert(w.id.clone()) || !w.path.is_absolute() { bail!("Workspaces need unique simple IDs and absolute root paths"); }
            if w.projects.is_empty() && (w.allow_exec==Some(true) || w.allow_git_commit==Some(true)) && w.allow_write!=Some(true) { bail!("Legacy workspace {}: execution/commit also require allow_write", w.id); }
            let mut projects=HashSet::new();
            for p in &w.projects {
                if !valid_id(&p.id)||!projects.insert(p.id.clone())||p.path.is_absolute()||p.path.components().any(|c|matches!(c,std::path::Component::ParentDir|std::path::Component::RootDir|std::path::Component::Prefix(_))){bail!("Workspace {} projects need unique simple IDs and relative paths without '..'",w.id);}
                if (p.allow_exec||p.allow_git_commit)&&!p.allow_write{bail!("Project {}/{}: execution/commit also require allow_write",w.id,p.id);}
            }
        }
        Ok(())
    }
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn defaults_are_protected() { let c = Config::default(); assert!(c.validate().is_ok()); assert_eq!(c.execution.backend, "bubblewrap"); assert!(!c.execution.allow_network); assert!(!c.execution.allow_shell); }
    #[test] fn host_mode_needs_acknowledgement() { let mut c = Config::default(); c.execution.backend = "host".into(); assert!(c.validate().is_err()); }
    #[test] fn http_public_server_is_rejected() { let mut c = Config::default(); c.server.public_url = "http://example.com".into(); assert!(c.validate().is_err()); }
    #[test] fn ids_are_bounded() { assert!(valid_id("BAfter")); assert!(!valid_id("../BAfter")); assert!(!valid_id("")); }
    #[test] fn projects_are_relative_to_workspace_roots(){let mut c=Config::default();c.workspaces=vec![WorkspaceConfig{id:"root".into(),path:"/tmp/root".into(),projects:vec![ProjectConfig{id:"app".into(),path:"app".into(),allow_write:true,allow_exec:true,allow_git_commit:true}],allow_write:None,allow_exec:None,allow_git_commit:None}];assert!(c.validate().is_ok());c.workspaces[0].projects[0].path="/tmp/root/app".into();assert!(c.validate().is_err());}
}
