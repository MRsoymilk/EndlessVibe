use crate::{config::{Config,ProjectConfig,WorkspaceConfig},security::paths::Root};
use anyhow::{bail,Context,Result};
use serde_json::{json,Value};
use std::{collections::BTreeMap,path::{Path,PathBuf},sync::Arc};
use tokio::sync::Mutex;

pub struct Project{pub workspace_id:String,pub config:ProjectConfig,pub root:Root,pub lock:Arc<Mutex<()>>}
impl Project{
    pub fn write_allowed(&self)->Result<()>{if !self.config.allow_write{bail!("Project is read-only");}self.root.unchanged_root()}
    pub fn exec_allowed(&self)->Result<()>{self.write_allowed()?;if !self.config.allow_exec{bail!("Command execution is disabled for this project");}Ok(())}
    pub fn commit_allowed(&self)->Result<()>{self.write_allowed()?;if !self.config.allow_git_commit{bail!("Git commits are disabled for this project");}Ok(())}
    pub fn push_allowed(&self)->Result<()>{if !self.config.allow_git_push{bail!("Git push is disabled for this project");}self.root.unchanged_root()}
    pub fn summary(&self)->Value{json!({"workspace":self.workspace_id,"id":self.config.id,"path":self.root.path,"allow_write":self.config.allow_write,"allow_exec":self.config.allow_exec,"allow_git_commit":self.config.allow_git_commit,"allow_git_mutation":self.config.allow_git_mutation,"allow_git_push":self.config.allow_git_push})}
}
pub struct Workspace{pub config:WorkspaceConfig,pub root:Root,pub projects:BTreeMap<String,Arc<Project>>}
impl Workspace{
    pub fn summary(&self)->Value{json!({"id":self.config.id,"path":self.root.path,"project_count":self.projects.len()})}
    pub fn project(&self,id:&str)->Result<Arc<Project>>{self.projects.get(id).cloned().with_context(||format!("PROJECT_NOT_AUTHORIZED: workspace {} has no project {id}",self.config.id))}
    pub fn projects_summary(&self)->Value{json!({"workspace":self.config.id,"projects":self.projects.values().map(|p|p.summary()).collect::<Vec<_>>()})}
}
fn same_root(a:&Root,b:&Root)->Result<bool>{use std::os::unix::fs::MetadataExt;let a=a.metadata(".")?;let b=b.metadata(".")?;Ok(a.dev()==b.dev()&&a.ino()==b.ino())}
fn legacy(w:&WorkspaceConfig)->bool{w.allow_write.is_some()||w.allow_exec.is_some()||w.allow_git_commit.is_some()||w.allow_git_mutation.is_some()||w.allow_git_push.is_some()}
fn project_config(id:String,path:PathBuf,w:&WorkspaceConfig)->ProjectConfig{ProjectConfig{id,path,allow_write:w.allow_write.unwrap_or(false),allow_exec:w.allow_exec.unwrap_or(false),allow_git_commit:w.allow_git_commit.unwrap_or(false),allow_git_mutation:w.allow_git_mutation.unwrap_or(false),allow_git_push:w.allow_git_push.unwrap_or(false)}}
fn insert_project(map:&mut BTreeMap<String,Arc<Project>>,workspace_id:&str,root_path:&Path,config:ProjectConfig,legacy_overlap:bool)->Result<()>{
    if map.contains_key(&config.id){bail!("Duplicate project ID {}/{}",workspace_id,config.id);}
    let host=if config.path==Path::new("."){root_path.to_owned()}else{root_path.join(&config.path)};
    let root=Root::open(&host).with_context(||format!("Cannot authorize project {workspace_id}/{}",config.id))?;
    if !root.path.starts_with(root_path){bail!("Project {workspace_id}/{} escapes its workspace root",config.id);}
    let mut lock=None;
    for other in map.values(){
        if same_root(&root,&other.root)?{bail!("Multiple project IDs cannot reference the same directory");}
        let overlaps=root.path.starts_with(&other.root.path)||other.root.path.starts_with(&root.path);
        if overlaps{
            if !legacy_overlap{bail!("Projects inside one workspace must not overlap: {}/{} and {}/{}",workspace_id,config.id,workspace_id,other.config.id);}
            lock=Some(other.lock.clone());
        }
    }
    map.insert(config.id.clone(),Arc::new(Project{workspace_id:workspace_id.into(),config,root,lock:lock.unwrap_or_else(||Arc::new(Mutex::new(())))}));
    Ok(())
}
pub fn load(config:&Config,config_path:&Path)->Result<BTreeMap<String,Arc<Workspace>>>{
    let state=config.security.data_dir.canonicalize()?;
    let cfg=config_path.canonicalize().unwrap_or_else(|_|config_path.to_owned());
    let mut opened:Vec<(WorkspaceConfig,Root)>=Vec::new();
    for item in &config.workspaces{
        let root=Root::open(&item.path).with_context(||format!("Cannot authorize workspace {}",item.id))?;
        if state.starts_with(&root.path)||root.path.starts_with(&state)||cfg.starts_with(&root.path){bail!("Workspace {} overlaps protected config/state directories",item.id);}
        for (_,other) in &opened{if same_root(&root,other)?{bail!("Multiple workspace IDs cannot reference the same directory");}}
        opened.push((item.clone(),root));
    }
    opened.sort_by_key(|(_,root)|root.path.components().count());
    let mut legacy_parent:Vec<Option<usize>>=vec![None;opened.len()];
    for i in 0..opened.len(){
        if !legacy(&opened[i].0)||!opened[i].0.projects.is_empty(){continue;}
        for j in (0..i).rev(){if opened[i].1.path.starts_with(&opened[j].1.path)&&legacy(&opened[j].0){legacy_parent[i]=Some(j);break;}}
    }
    let mut out=BTreeMap::new();
    for i in 0..opened.len(){
        if legacy_parent[i].is_some(){continue;}
        let item=opened[i].0.clone();let root_path=opened[i].1.path.clone();
        for (other,other_root) in &opened{
            if other.id!=item.id&&!(legacy(&item)&&legacy(other))&&(root_path.starts_with(&other_root.path)||other_root.path.starts_with(&root_path)){bail!("Configured workspace roots must not overlap");}
        }
        let root=Root::open(&root_path)?;
        let mut projects=BTreeMap::new();
        let mut legacy_children=Vec::new();
        for k in 0..opened.len(){
            let mut parent=legacy_parent[k];let mut belongs=false;
            while let Some(p)=parent{if p==i{belongs=true;break;}parent=legacy_parent[p];}
            if belongs{legacy_children.push(k);}
        }
        if legacy(&item)&&item.projects.is_empty()&&legacy_children.is_empty(){insert_project(&mut projects,&item.id,&root_path,project_config(item.id.clone(),PathBuf::from("."),&item),true)?;}
        for p in &item.projects{insert_project(&mut projects,&item.id,&root_path,p.clone(),false)?;}
        for k in legacy_children{
            let child=&opened[k];let rel=child.1.path.strip_prefix(&root_path).context("Legacy project is outside workspace root")?.to_owned();
            insert_project(&mut projects,&item.id,&root_path,project_config(child.0.id.clone(),rel,&child.0),true)?;
        }
        out.insert(item.id.clone(),Arc::new(Workspace{config:item,root,projects}));
    }
    Ok(out)
}
pub fn inspect(project:&Project)->Result<Value>{
    let mut languages=Vec::new();let mut build=Vec::new();
    for (marker,language,system) in [("Cargo.toml","Rust","Cargo"),("CMakeLists.txt","C/C++","CMake"),("pyproject.toml","Python","pyproject"),("package.json","JavaScript/TypeScript","npm"),("Makefile","unknown","Make")]{
        if project.root.metadata(marker).is_ok(){languages.push(language);build.push(system);}
    }
    let entries=project.root.entries(".")?.into_iter().take(200).map(|e|json!({"name":e.name,"type":e.kind})).collect::<Vec<_>>();
    Ok(json!({"workspace":project.workspace_id,"project":project.summary(),"languages":languages,"build_systems":build,"entries":entries,"note":"Detection uses file names only; no project code was executed."}))
}

#[cfg(test)]mod tests{
    use super::*;
    fn legacy(id:&str,path:&Path)->WorkspaceConfig{WorkspaceConfig{id:id.into(),path:path.into(),projects:vec![],allow_write:Some(true),allow_exec:Some(true),allow_git_commit:Some(true),allow_git_mutation:Some(false),allow_git_push:Some(false)}}
    fn nested(id:&str,path:&Path,projects:Vec<ProjectConfig>)->WorkspaceConfig{WorkspaceConfig{id:id.into(),path:path.into(),projects,allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None}}
    fn project(id:&str,path:&str)->ProjectConfig{ProjectConfig{id:id.into(),path:path.into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false}}
    #[test]fn legacy_parent_becomes_root_for_child_projects(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let root=t.path().join("root");let a=root.join("a");let b=root.join("b");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(&a).unwrap();std::fs::create_dir_all(&b).unwrap();let cfg_path=t.path().join("config.toml");std::fs::write(&cfg_path,"").unwrap();let mut c=Config::default();c.security.data_dir=state;c.workspaces=vec![legacy("NAME",&root),legacy("a",&a),legacy("b",&b)];let m=load(&c,&cfg_path).unwrap();assert_eq!(m.len(),1);assert!(!m["NAME"].projects.contains_key("NAME"));assert!(m["NAME"].projects.contains_key("a"));assert!(m["NAME"].projects.contains_key("b"));assert!(!Arc::ptr_eq(&m["NAME"].projects["a"].lock,&m["NAME"].projects["b"].lock));}
    #[test]fn standalone_legacy_workspace_remains_compatible_project(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let root=t.path().join("root");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(&root).unwrap();let cfg_path=t.path().join("config.toml");std::fs::write(&cfg_path,"").unwrap();let mut c=Config::default();c.security.data_dir=state;c.workspaces=vec![legacy("solo",&root)];let m=load(&c,&cfg_path).unwrap();assert!(m["solo"].projects.contains_key("solo"));}
    #[test]fn configured_projects_get_independent_locks(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let root=t.path().join("root");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(root.join("a")).unwrap();std::fs::create_dir_all(root.join("b")).unwrap();let cfg_path=t.path().join("config.toml");std::fs::write(&cfg_path,"").unwrap();let mut c=Config::default();c.security.data_dir=state;c.workspaces=vec![nested("root",&root,vec![project("a","a"),project("b","b")])];let m=load(&c,&cfg_path).unwrap();assert!(!Arc::ptr_eq(&m["root"].projects["a"].lock,&m["root"].projects["b"].lock));}
    #[test]fn configured_project_cannot_escape_or_overlap(){let t=tempfile::tempdir().unwrap();let state=t.path().join("state");let root=t.path().join("root");std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(root.join("a/sub")).unwrap();let cfg_path=t.path().join("config.toml");std::fs::write(&cfg_path,"").unwrap();let mut c=Config::default();c.security.data_dir=state;c.workspaces=vec![nested("root",&root,vec![project("a","a"),project("sub","a/sub")])];assert!(load(&c,&cfg_path).is_err());}
}
