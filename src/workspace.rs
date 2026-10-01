use crate::{config::{Config, WorkspaceConfig}, security::paths::Root};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::sync::Mutex;

pub struct Workspace { pub config: WorkspaceConfig, pub root: Root, pub lock: Arc<Mutex<()>> }
impl Workspace {
    pub fn write_allowed(&self) -> Result<()> { if !self.config.allow_write { bail!("Workspace is read-only"); } self.root.unchanged_root() }
    pub fn exec_allowed(&self) -> Result<()> { self.write_allowed()?; if !self.config.allow_exec { bail!("Command execution is disabled for this workspace"); } Ok(()) }
    pub fn commit_allowed(&self) -> Result<()> { self.write_allowed()?; if !self.config.allow_git_commit { bail!("Git commits are disabled for this workspace"); } Ok(()) }
    pub fn summary(&self) -> Value { json!({"id":self.config.id,"path":self.root.path,"allow_write":self.config.allow_write,"allow_exec":self.config.allow_exec,"allow_git_commit":self.config.allow_git_commit,"allow_git_push":false}) }
}
pub fn load(config: &Config, config_path: &Path) -> Result<BTreeMap<String, Arc<Workspace>>> {
    use std::os::unix::fs::MetadataExt;
    let state = config.security.data_dir.canonicalize()?;
    let cfg = config_path.canonicalize().unwrap_or_else(|_|config_path.to_owned());
    let mut opened:Vec<(WorkspaceConfig,Root)>=Vec::new();
    for item in &config.workspaces {
        let root = Root::open(&item.path).with_context(|| format!("Cannot authorize workspace {}", item.id))?;
        if state.starts_with(&root.path) || root.path.starts_with(&state) || cfg.starts_with(&root.path) { bail!("Workspace {} overlaps the protected config/state directories", item.id); }
        let a=root.metadata(".")?;for (_,other) in &opened{let b=other.metadata(".")?;if a.dev()==b.dev()&&a.ino()==b.ino(){bail!("Multiple workspace IDs cannot reference the same directory");}}
        opened.push((item.clone(),root));
    }
    opened.sort_by_key(|(_,root)|root.path.components().count());
    let mut map: BTreeMap<String, Arc<Workspace>> = BTreeMap::new();
    for (item,root) in opened {
        let lock=map.values().find(|other|root.path.starts_with(&other.root.path)||other.root.path.starts_with(&root.path)).map(|other|other.lock.clone()).unwrap_or_else(||Arc::new(Mutex::new(())));
        map.insert(item.id.clone(), Arc::new(Workspace { config: item, root, lock }));
    }
    Ok(map)
}
pub fn inspect(workspace: &Workspace) -> Result<Value> {
    let mut languages = Vec::new(); let mut build = Vec::new();
    for (marker, language, system) in [("Cargo.toml","Rust","Cargo"),("CMakeLists.txt","C/C++","CMake"),("pyproject.toml","Python","pyproject"),("package.json","JavaScript/TypeScript","npm"),("Makefile","unknown","Make")] {
        if workspace.root.metadata(marker).is_ok() { languages.push(language); build.push(system); }
    }
    let entries = workspace.root.entries(".")?.into_iter().take(200).map(|e| json!({"name":e.name,"type":e.kind})).collect::<Vec<_>>();
    Ok(json!({"workspace":workspace.summary(),"languages":languages,"build_systems":build,"entries":entries,"note":"Detection uses file names only; no project code was executed."}))
}

#[cfg(test)]mod tests{
    use super::*;
    fn wc(id:&str,path:&Path)->WorkspaceConfig{WorkspaceConfig{id:id.into(),path:path.into(),allow_write:true,allow_exec:true,allow_git_commit:true}}
    #[test]fn nested_workspaces_share_lock(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let parent=t.path().join("project");let child=parent.join("child");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(&child).unwrap();let cfg_path=t.path().join("config.toml");std::fs::write(&cfg_path,"").unwrap();let mut c=Config::default();c.security.data_dir=state;c.workspaces=vec![wc("child",&child),wc("parent",&parent)];let m=load(&c,&cfg_path).unwrap();assert!(Arc::ptr_eq(&m["parent"].lock,&m["child"].lock));}
    #[test]fn same_directory_alias_is_rejected(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let project=t.path().join("project");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(&project).unwrap();let cfg_path=t.path().join("config.toml");std::fs::write(&cfg_path,"").unwrap();let mut c=Config::default();c.security.data_dir=state;c.workspaces=vec![wc("one",&project),wc("two",&project)];assert!(load(&c,&cfg_path).is_err());}
}
