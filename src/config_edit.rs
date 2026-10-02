use crate::{config::{self,Config,ProjectConfig},util,workspace};
use anyhow::{bail,Context,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{os::unix::fs::MetadataExt,path::{Path,PathBuf}};

#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddProjectRequest{
    pub expected_revision:String,
    pub workspace:String,
    #[serde(default)]pub project:Option<String>,
    pub path:String,
    #[serde(default="yes")]pub allow_write:bool,
    #[serde(default="yes")]pub allow_exec:bool,
    #[serde(default="yes")]pub allow_git_commit:bool,
    #[serde(default)]pub allow_git_mutation:bool,
    #[serde(default)]pub allow_git_push:bool,
}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProjectRequest{
    pub expected_revision:String,
    pub allow_write:bool,
    pub allow_exec:bool,
    pub allow_git_commit:bool,
    #[serde(default)]pub allow_git_mutation:bool,
    #[serde(default)]pub allow_git_push:bool,
}
#[derive(Clone,Debug,Serialize)]
pub struct ConfigMutation{
    pub revision:String,
    pub requires_restart:bool,
    pub project:Value,
    #[serde(skip_serializing_if="Option::is_none")]pub reload_warning:Option<String>,
    #[serde(rename="_operation_diff")]
    pub operation_diff:String,
}
fn yes()->bool{true}
fn config_diff(old:&str,new:&str)->String{if old==new{return String::new();}let old_lines=old.split_inclusive('\n').collect::<Vec<_>>();let new_lines=new.split_inclusive('\n').collect::<Vec<_>>();let mut prefix=0usize;while prefix<old_lines.len()&&prefix<new_lines.len()&&old_lines[prefix]==new_lines[prefix]{prefix+=1;}let mut suffix=0usize;while suffix<old_lines.len().saturating_sub(prefix)&&suffix<new_lines.len().saturating_sub(prefix)&&old_lines[old_lines.len()-1-suffix]==new_lines[new_lines.len()-1-suffix]{suffix+=1;}let context=3usize;let old_start=prefix.saturating_sub(context);let new_start=prefix.saturating_sub(context);let old_change_end=old_lines.len().saturating_sub(suffix);let new_change_end=new_lines.len().saturating_sub(suffix);let old_end=(old_change_end+context).min(old_lines.len());let new_end=(new_change_end+context).min(new_lines.len());let mut diff=format!("--- a/config.toml\n+++ b/config.toml\n@@ -{},{} +{},{} @@\n",old_start+1,old_end-old_start,new_start+1,new_end-new_start);for line in &old_lines[old_start..prefix]{diff.push(' ');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}for line in &old_lines[prefix..old_change_end]{diff.push('-');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}for line in &new_lines[prefix..new_change_end]{diff.push('+');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}for line in &old_lines[old_change_end..old_end]{diff.push(' ');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}if diff.len()>131072{let mut end=131072;while end>0&&!diff.is_char_boundary(end){end-=1;}diff.truncate(end);diff.push_str("\n… [diff truncated]\n");}diff}

pub fn revision(path:&Path)->Result<String>{Ok(util::digest(std::fs::read(path).with_context(||format!("Read {}",path.display()))?))}
fn read_checked(path:&Path,expected_revision:&str)->Result<(String,Config)>{
    let bytes=std::fs::read(path).with_context(||format!("Read {}",path.display()))?;
    let actual=util::digest(&bytes);
    if actual!=expected_revision{return Err(crate::error::coded_details("CONFIG_CONFLICT",true,"configuration changed since it was loaded; refresh and retry",json!({"expected_revision":expected_revision,"actual_revision":actual})));}
    let text=String::from_utf8(bytes).context("Config must be UTF-8")?;
    let cfg:Config=toml::from_str(&text).context("Invalid config.toml")?;
    cfg.validate()?;
    Ok((text,cfg))
}
fn replace_config(path:&Path,bytes:&[u8])->Result<()>{
    let md=std::fs::symlink_metadata(path).with_context(||format!("Read metadata for {}",path.display()))?;
    if !md.is_file()||md.file_type().is_symlink()||md.nlink()!=1||md.uid()!=unsafe{libc::geteuid()}{bail!("Config must be a regular, single-link file owned by the service user");}
    let parent=path.parent().context("Config needs a parent directory")?;
    let name=path.file_name().and_then(|v|v.to_str()).unwrap_or("config.toml");
    let temp=parent.join(format!(".{name}.{}.tmp",util::random_secret()?));
    util::private_create(&temp,bytes)?;
    if let Err(error)=std::fs::rename(&temp,path){let _=std::fs::remove_file(&temp);return Err(error).with_context(||format!("Replace {}",path.display()));}
    Ok(())
}
fn workspace_table_offsets(text:&str)->Vec<usize>{
    let mut out=Vec::new();let mut offset=0;
    for line in text.split_inclusive('\n'){if line.trim()=="[[workspaces]]"{out.push(offset);}offset+=line.len();}
    out
}
fn insert_project_text(text:&mut String,workspace_index:usize,project:&ProjectConfig)->Result<()>{
    let offsets=workspace_table_offsets(text);
    let insert=if workspace_index+1<offsets.len(){offsets[workspace_index+1]}else{text.len()};
    let mut block=String::new();
    if insert>0&&!text[..insert].ends_with('\n'){block.push('\n');}
    block.push_str("\n[[workspaces.projects]]\n");
    block.push_str(&toml::to_string(project)?);
    text.insert_str(insert,&block);
    Ok(())
}
fn workspace_block_range(text:&str,workspace_index:usize)->Result<(usize,usize)>{let workspaces=workspace_table_offsets(text);let start=*workspaces.get(workspace_index).context("Workspace table is missing from config text")?;let end=workspaces.get(workspace_index+1).copied().unwrap_or(text.len());Ok((start,end))}
fn project_block_range(text:&str,workspace_index:usize,project_index:usize)->Result<(usize,usize)>{
    let workspaces=workspace_table_offsets(text);
    let start=*workspaces.get(workspace_index).context("Workspace table is missing from config text")?;
    let end=workspaces.get(workspace_index+1).copied().unwrap_or(text.len());
    let segment=&text[start..end];
    let marker="[[workspaces.projects]]";
    let mut offsets=segment.match_indices(marker).map(|(offset,_)|start+offset).collect::<Vec<_>>();
    let block_start=*offsets.get(project_index).context("Project table is missing from config text")?;
    offsets.push(end);
    Ok((block_start,offsets[project_index+1]))
}
fn replace_bool(block:&mut String,key:&str,value:bool)->Result<()>{
    let mut offset=0usize;let mut found=None;
    for line in block.split_inclusive('\n'){
        let trimmed=line.trim_start();
        if trimmed.starts_with(&format!("{key} =")){found=Some((offset,offset+line.len(),line.len()-trimmed.len()));break;}
        offset+=line.len();
    }
    let replacement=if let Some((start,end,indent))=found{
        let suffix=if block[start..end].ends_with('\n'){"\n"}else{""};
        (start,end,format!("{}{key} = {value}{suffix}"," ".repeat(indent)))
    }else{
        let pos=block.len();let prefix=if block.ends_with('\n'){""}else{"\n"};
        (pos,pos,format!("{prefix}{key} = {value}\n"))
    };
    block.replace_range(replacement.0..replacement.1,&replacement.2);
    Ok(())
}
fn validate_project_permissions(write:bool,exec:bool,commit:bool,mutation:bool)->Result<()>{
    if (exec||commit)&&!write{bail!("Project execution/commit require allow_write=true");}
    if mutation&&(!write||!exec){bail!("allow_git_mutation requires allow_write=true and allow_exec=true");}
    Ok(())
}
fn project_value(workspace:&str,project:&ProjectConfig,root:&Path)->Value{
    json!({"workspace":workspace,"id":project.id,"path":root.join(&project.path),"allow_write":project.allow_write,"allow_exec":project.allow_exec,"allow_git_commit":project.allow_git_commit,"allow_git_mutation":project.allow_git_mutation,"allow_git_push":project.allow_git_push})
}
pub fn add_project(path:&Path,request:AddProjectRequest)->Result<ConfigMutation>{
    validate_project_permissions(request.allow_write,request.allow_exec,request.allow_git_commit,request.allow_git_mutation)?;
    let (mut text,mut cfg)=read_checked(path,&request.expected_revision)?;let original=text.clone();
    if !config::valid_id(&request.workspace){bail!("Invalid workspace ID");}
    let workspace_index=cfg.workspaces.iter().position(|w|w.id==request.workspace).with_context(||format!("Workspace {} is not configured",request.workspace))?;
    let root=cfg.workspaces[workspace_index].path.canonicalize().with_context(||format!("Cannot resolve workspace root {}",cfg.workspaces[workspace_index].path.display()))?;
    let raw=PathBuf::from(&request.path);if !raw.is_absolute(){bail!("Project path must be absolute");}
    let absolute=raw.canonicalize().with_context(||format!("Cannot resolve project path {}",raw.display()))?;
    if absolute==root||!absolute.starts_with(&root){bail!("Project must be a strict descendant of workspace {} ({})",request.workspace,root.display());}
    let inferred=absolute.file_name().and_then(|v|v.to_str()).filter(|v|!v.is_empty()).context("Cannot infer project ID")?;
    let id=request.project.as_deref().filter(|v|!v.is_empty()).unwrap_or(inferred);
    if !config::valid_id(id){bail!("Project ID must contain only ASCII letters, digits, '_' or '-' and be at most 64 characters");}
    if cfg.workspaces[workspace_index].projects.iter().any(|p|p.id==id){bail!("Project ID {}/{} already exists",request.workspace,id);}
    for existing in &cfg.workspaces[workspace_index].projects{
        let other=root.join(&existing.path).canonicalize().with_context(||format!("Cannot resolve configured project {}/{}",request.workspace,existing.id))?;
        if absolute==other||absolute.starts_with(&other)||other.starts_with(&absolute){bail!("Project paths inside one workspace must not overlap");}
    }
    let project=ProjectConfig{id:id.into(),path:absolute.strip_prefix(&root)?.to_owned(),allow_write:request.allow_write,allow_exec:request.allow_exec,allow_git_commit:request.allow_git_commit,allow_git_mutation:request.allow_git_mutation,allow_git_push:request.allow_git_push};
    cfg.workspaces[workspace_index].projects.push(project.clone());cfg.validate()?;drop(workspace::load(&cfg,path)?);
    insert_project_text(&mut text,workspace_index,&project)?;
    let verify:Config=toml::from_str(&text).context("Generated project config is invalid")?;verify.validate()?;drop(workspace::load(&verify,path)?);
    replace_config(path,text.as_bytes())?;
    Ok(ConfigMutation{revision:util::digest(text.as_bytes()),requires_restart:true,project:project_value(&request.workspace,&project,&root),reload_warning:None,operation_diff:config_diff(&original,&text)})
}
pub fn update_project(path:&Path,workspace_id:&str,project_id:&str,request:UpdateProjectRequest)->Result<ConfigMutation>{
    validate_project_permissions(request.allow_write,request.allow_exec,request.allow_git_commit,request.allow_git_mutation)?;
    let (mut text,mut cfg)=read_checked(path,&request.expected_revision)?;let original=text.clone();
    let workspace_index=cfg.workspaces.iter().position(|w|w.id==workspace_id).with_context(||format!("Workspace {workspace_id} is not configured"))?;
    let root=cfg.workspaces[workspace_index].path.canonicalize()?;
    let direct=cfg.workspaces[workspace_index].projects.iter().position(|p|p.id==project_id);
    let project=if let Some(project_index)=direct{
        {
            let project=&mut cfg.workspaces[workspace_index].projects[project_index];
            project.allow_write=request.allow_write;project.allow_exec=request.allow_exec;project.allow_git_commit=request.allow_git_commit;project.allow_git_mutation=request.allow_git_mutation;project.allow_git_push=request.allow_git_push;
        }
        let (start,end)=project_block_range(&text,workspace_index,project_index)?;let mut block=text[start..end].to_owned();
        for(key,value)in [("allow_write",request.allow_write),("allow_exec",request.allow_exec),("allow_git_commit",request.allow_git_commit),("allow_git_mutation",request.allow_git_mutation),("allow_git_push",request.allow_git_push)]{replace_bool(&mut block,key,value)?;}text.replace_range(start..end,&block);
        cfg.workspaces[workspace_index].projects[project_index].clone()
    }else{
        let legacy_index=cfg.workspaces.iter().enumerate().find_map(|(index,w)|{
            if index==workspace_index||w.id!=project_id||!w.projects.is_empty()||!(w.allow_write.is_some()||w.allow_exec.is_some()||w.allow_git_commit.is_some()||w.allow_git_mutation.is_some()||w.allow_git_push.is_some()){return None;}
            let child=w.path.canonicalize().ok()?;if child==root||!child.starts_with(&root){return None;}Some(index)
        }).with_context(||format!("Project {workspace_id}/{project_id} is not configured"))?;
        let relative=cfg.workspaces[legacy_index].path.canonicalize()?.strip_prefix(&root)?.to_owned();
        {
            let legacy=&mut cfg.workspaces[legacy_index];legacy.allow_write=Some(request.allow_write);legacy.allow_exec=Some(request.allow_exec);legacy.allow_git_commit=Some(request.allow_git_commit);legacy.allow_git_mutation=Some(request.allow_git_mutation);legacy.allow_git_push=Some(request.allow_git_push);
        }
        let (start,end)=workspace_block_range(&text,legacy_index)?;let mut block=text[start..end].to_owned();
        for(key,value)in [("allow_write",request.allow_write),("allow_exec",request.allow_exec),("allow_git_commit",request.allow_git_commit),("allow_git_mutation",request.allow_git_mutation),("allow_git_push",request.allow_git_push)]{replace_bool(&mut block,key,value)?;}text.replace_range(start..end,&block);
        ProjectConfig{id:project_id.into(),path:relative,allow_write:request.allow_write,allow_exec:request.allow_exec,allow_git_commit:request.allow_git_commit,allow_git_mutation:request.allow_git_mutation,allow_git_push:request.allow_git_push}
    };
    cfg.validate()?;drop(workspace::load(&cfg,path)?);
    let verify:Config=toml::from_str(&text).context("Generated project config is invalid")?;verify.validate()?;drop(workspace::load(&verify,path)?);
    replace_config(path,text.as_bytes())?;
    Ok(ConfigMutation{revision:util::digest(text.as_bytes()),requires_restart:true,project:project_value(workspace_id,&project,&root),reload_warning:None,operation_diff:config_diff(&original,&text)})
}

#[cfg(test)]
mod tests{
    use super::*;
    fn config(root:&Path,state:&Path)->Config{let mut c=Config::default();c.security.data_dir=state.into();c.workspaces=vec![crate::config::WorkspaceConfig{id:"root".into(),path:root.into(),projects:vec![],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None}];c}
    #[test]fn add_and_update_preserve_comments(){let t=tempfile::tempdir().unwrap();let root=t.path().join("root");let child=root.join("demo");let state=t.path().join("state");std::fs::create_dir_all(&child).unwrap();std::fs::create_dir_all(&state).unwrap();let path=t.path().join("config.toml");let mut text=toml::to_string_pretty(&config(&root,&state)).unwrap();text.push_str("\n# keep-comment\n");std::fs::write(&path,&text).unwrap();let rev=revision(&path).unwrap();let added=add_project(&path,AddProjectRequest{expected_revision:rev,workspace:"root".into(),project:None,path:child.display().to_string(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false}).unwrap();let updated=update_project(&path,"root","demo",UpdateProjectRequest{expected_revision:added.revision,allow_write:true,allow_exec:false,allow_git_commit:false,allow_git_mutation:false,allow_git_push:false}).unwrap();assert!(updated.requires_restart);let after=std::fs::read_to_string(&path).unwrap();assert!(after.contains("# keep-comment"));let loaded=Config::load_file(&path).unwrap();assert!(!loaded.workspaces[0].projects[0].allow_exec);}
    #[test]fn legacy_child_project_permissions_are_editable(){let t=tempfile::tempdir().unwrap();let root=t.path().join("projects");let child=root.join("EndlessVibe");let state=t.path().join("state");std::fs::create_dir_all(&child).unwrap();std::fs::create_dir_all(&state).unwrap();let path=t.path().join("config.toml");let mut cfg=Config::default();cfg.security.data_dir=state;cfg.workspaces=vec![crate::config::WorkspaceConfig{id:"NAME".into(),path:root.clone(),projects:vec![],allow_write:Some(true),allow_exec:Some(true),allow_git_commit:Some(true),allow_git_mutation:Some(false),allow_git_push:Some(false)},crate::config::WorkspaceConfig{id:"EndlessVibe".into(),path:child,projects:vec![],allow_write:Some(true),allow_exec:Some(true),allow_git_commit:Some(true),allow_git_mutation:Some(false),allow_git_push:Some(false)}];std::fs::write(&path,toml::to_string_pretty(&cfg).unwrap()).unwrap();let rev=revision(&path).unwrap();update_project(&path,"NAME","EndlessVibe",UpdateProjectRequest{expected_revision:rev,allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:true,allow_git_push:true}).unwrap();let loaded=Config::load_file(&path).unwrap();let legacy=loaded.workspaces.iter().find(|w|w.id=="EndlessVibe").unwrap();assert_eq!(legacy.allow_git_mutation,Some(true));assert_eq!(legacy.allow_git_push,Some(true));let effective=workspace::load(&loaded,&path).unwrap();assert!(effective["NAME"].projects["EndlessVibe"].config.allow_git_mutation);assert!(effective["NAME"].projects["EndlessVibe"].config.allow_git_push);}
}
