use crate::{config::{self,Config,ProjectConfig,ReadOnlyMount},util,workspace};
use anyhow::{bail,Context,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::path::{Path,PathBuf};

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
    #[serde(default)]pub execution_profile:Option<String>,
    #[serde(default)]pub environment:Vec<String>,
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
    #[serde(default)]pub execution_profile:Option<String>,
    #[serde(default)]pub environment:Option<Vec<String>>,
}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateReadonlyMountsRequest{pub expected_revision:String,#[serde(default)]pub readonly_mounts:Vec<ReadOnlyMount>}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateGitRequest{pub expected_revision:String,pub executable:String,pub author_name:String,pub author_email:String}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateLimitsRequest{pub expected_revision:String,pub max_file_bytes:usize,pub max_read_bytes:usize,pub max_output_bytes:usize,pub command_timeout_seconds:u64,pub max_jobs:usize,pub retained_jobs:usize,pub search_max_files:usize,pub search_max_bytes:usize}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateDockerRequest{pub expected_revision:String,pub enabled:bool,pub socket:String,pub allowed_containers:Vec<String>,pub allow_start:bool,pub allow_stop:bool,pub allow_restart:bool,pub allow_project_socket:bool,pub acknowledge_daemon_control:bool}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateTransferRequest{pub expected_revision:String,pub enabled:bool,pub listen:String,pub advertise:bool,pub discover:bool,pub display_name:String}
#[derive(Clone,Debug,Serialize)]
pub struct ServiceConfigMutation{pub revision:String,pub requires_restart:bool,pub execution:Value,#[serde(rename="_operation_diff")]pub operation_diff:String}
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
fn redact_config_diff(diff:&str)->String{let mut out=String::with_capacity(diff.len());for line in diff.split_inclusive('\n'){let newline=line.ends_with('\n');let raw=line.strip_suffix('\n').unwrap_or(line);let(prefix,body)=match raw.as_bytes().first(){Some(b'+'|b'-'|b' ')=>( &raw[..1],&raw[1..]),_=>("",raw)};if body.trim_start().starts_with("environment ="){out.push_str(prefix);out.push_str("environment = [REDACTED]");}else{out.push_str(raw);}if newline{out.push('\n');}}out}
fn config_diff(old:&str,new:&str)->String{if old==new{return String::new();}let old_lines=old.split_inclusive('\n').collect::<Vec<_>>();let new_lines=new.split_inclusive('\n').collect::<Vec<_>>();let mut prefix=0usize;while prefix<old_lines.len()&&prefix<new_lines.len()&&old_lines[prefix]==new_lines[prefix]{prefix+=1;}let mut suffix=0usize;while suffix<old_lines.len().saturating_sub(prefix)&&suffix<new_lines.len().saturating_sub(prefix)&&old_lines[old_lines.len()-1-suffix]==new_lines[new_lines.len()-1-suffix]{suffix+=1;}let context=3usize;let old_start=prefix.saturating_sub(context);let new_start=prefix.saturating_sub(context);let old_change_end=old_lines.len().saturating_sub(suffix);let new_change_end=new_lines.len().saturating_sub(suffix);let old_end=(old_change_end+context).min(old_lines.len());let new_end=(new_change_end+context).min(new_lines.len());let mut diff=format!("--- a/config.toml\n+++ b/config.toml\n@@ -{},{} +{},{} @@\n",old_start+1,old_end-old_start,new_start+1,new_end-new_start);for line in &old_lines[old_start..prefix]{diff.push(' ');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}for line in &old_lines[prefix..old_change_end]{diff.push('-');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}for line in &new_lines[prefix..new_change_end]{diff.push('+');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}for line in &old_lines[old_change_end..old_end]{diff.push(' ');diff.push_str(line);if !line.ends_with('\n'){diff.push('\n');}}diff=redact_config_diff(&diff);if diff.len()>131072{let mut end=131072;while end>0&&!diff.is_char_boundary(end){end-=1;}diff.truncate(end);diff.push_str("\n… [diff truncated]\n");}diff}

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
    if !md.is_file()||md.file_type().is_symlink()||!crate::platform::single_link_path(path)||!crate::platform::owned_by_service(&md){bail!("Config must be a regular, single-link file owned by the service user");}
    let parent=path.parent().context("Config needs a parent directory")?;
    let name=path.file_name().and_then(|v|v.to_str()).unwrap_or("config.toml");
    let temp=parent.join(format!(".{name}.{}.tmp",util::random_secret()?));
    util::private_create(&temp,bytes)?;
    if let Err(error)=std::fs::rename(&temp,path){let _=std::fs::remove_file(&temp);return Err(error).with_context(||format!("Replace {}",path.display()));}
    Ok(())
}
fn replace_execution_readonly_mounts(text:&mut String,mounts:&[ReadOnlyMount])->Result<()>{use toml_edit::{Array,DocumentMut,InlineTable,Value};let mut doc=text.parse::<DocumentMut>().context("Invalid config.toml for structured edit")?;if !doc["execution"].is_table(){bail!("Missing [execution] table in config");}let mut array=Array::new();for mount in mounts{let source=mount.source.to_str().context("readonly mount source must be UTF-8")?;let target=mount.target.to_str().context("readonly mount target must be UTF-8")?;let mut item=InlineTable::new();item.insert("source",Value::from(source));item.insert("target",Value::from(target));array.push(Value::InlineTable(item));}doc["execution"]["readonly_mounts"]=toml_edit::value(array);*text=doc.to_string();Ok(())}
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
fn validate_project_permissions(write:bool,exec:bool,commit:bool,mutation:bool)->Result<()>{
    if (exec||commit)&&!write{bail!("Project execution/commit require allow_write=true");}
    if mutation&&(!write||!exec){bail!("allow_git_mutation requires allow_write=true and allow_exec=true");}
    Ok(())
}
fn project_value(workspace:&str,project:&ProjectConfig,root:&Path)->Value{
    let environment_keys=project.environment_map().map(|v|v.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();json!({"workspace":workspace,"id":project.id,"path":root.join(&project.path),"allow_write":project.allow_write,"allow_exec":project.allow_exec,"allow_git_commit":project.allow_git_commit,"allow_git_mutation":project.allow_git_mutation,"allow_git_push":project.allow_git_push,"execution_profile":project.execution_profile,"environment_keys":environment_keys})
}
fn edit_service_section(text:&mut String,section:&str,items:&[(&str,toml_edit::Value)])->Result<()>{use toml_edit::DocumentMut;let mut doc=text.parse::<DocumentMut>().context("Invalid config.toml for structured edit")?;if !doc.as_table().contains_key(section){doc.as_table_mut().insert(section,toml_edit::Item::Table(toml_edit::Table::new()));}if !doc[section].is_table(){bail!("[{section}] must be a table in config");}for(key,value)in items{doc[section][key]=toml_edit::Item::Value(value.clone());}*text=doc.to_string();Ok(())}
pub fn update_transfer(path:&Path,request:UpdateTransferRequest)->Result<Value>{let(mut text,mut cfg)=read_checked(path,&request.expected_revision)?;let original=text.clone();cfg.transfer=config::Transfer{enabled:request.enabled,listen:request.listen.parse().context("Invalid Transfer IP:port listener")?,advertise:request.advertise,discover:request.discover,display_name:request.display_name.clone()};cfg.validate()?;edit_service_section(&mut text,"transfer",&[("enabled",toml_edit::Value::from(cfg.transfer.enabled)),("listen",toml_edit::Value::from(request.listen)),("advertise",toml_edit::Value::from(request.advertise)),("discover",toml_edit::Value::from(request.discover)),("display_name",toml_edit::Value::from(request.display_name))])?;let verify:Config=toml::from_str(&text).context("Generated Transfer config is invalid")?;verify.validate()?;replace_config(path,text.as_bytes())?;Ok(json!({"revision":util::digest(text.as_bytes()),"requires_restart":true,"transfer":verify.transfer,"_operation_diff":config_diff(&original,&text)}))}
pub fn update_docker(path:&Path,request:UpdateDockerRequest)->Result<Value>{let(mut text,mut cfg)=read_checked(path,&request.expected_revision)?;let original=text.clone();if (request.enabled||request.allow_project_socket)&&!request.acknowledge_daemon_control{bail!("Enabling Docker access requires acknowledge_daemon_control=true; Docker socket access can control the host");}cfg.docker=config::Docker{enabled:request.enabled,socket:PathBuf::from(&request.socket),allowed_containers:request.allowed_containers.clone(),allow_start:request.allow_start,allow_stop:request.allow_stop,allow_restart:request.allow_restart,allow_project_socket:request.allow_project_socket};cfg.validate()?;let mut containers=toml_edit::Array::new();for name in &cfg.docker.allowed_containers{containers.push(name.as_str());}edit_service_section(&mut text,"docker",&[("enabled",toml_edit::Value::from(cfg.docker.enabled)),("socket",toml_edit::Value::from(cfg.docker.socket.to_string_lossy().as_ref())),("allowed_containers",toml_edit::Value::Array(containers)),("allow_start",toml_edit::Value::from(cfg.docker.allow_start)),("allow_stop",toml_edit::Value::from(cfg.docker.allow_stop)),("allow_restart",toml_edit::Value::from(cfg.docker.allow_restart)),("allow_project_socket",toml_edit::Value::from(cfg.docker.allow_project_socket))])?;let verified:Config=toml::from_str(&text).context("Generated Docker config is invalid")?;verified.validate()?;replace_config(path,text.as_bytes())?;Ok(json!({"revision":util::digest(text.as_bytes()),"requires_restart":true,"docker":verified.docker,"_operation_diff":config_diff(&original,&text)}))}
pub fn update_git(path:&Path,request:UpdateGitRequest)->Result<Value>{let(mut text,mut cfg)=read_checked(path,&request.expected_revision)?;let original=text.clone();cfg.git.executable=PathBuf::from(&request.executable);cfg.git.author_name=request.author_name.clone();cfg.git.author_email=request.author_email.clone();cfg.validate()?;edit_service_section(&mut text,"git",&[("executable",toml_edit::Value::from(request.executable)),("author_name",toml_edit::Value::from(request.author_name)),("author_email",toml_edit::Value::from(request.author_email))])?;let verify:Config=toml::from_str(&text).context("Generated Git config is invalid")?;verify.validate()?;replace_config(path,text.as_bytes())?;Ok(json!({"revision":util::digest(text.as_bytes()),"requires_restart":true,"git":verify.git,"_operation_diff":config_diff(&original,&text)}))}
pub fn update_limits(path:&Path,request:UpdateLimitsRequest)->Result<Value>{let(mut text,mut cfg)=read_checked(path,&request.expected_revision)?;let original=text.clone();cfg.limits=config::Limits{max_file_bytes:request.max_file_bytes,max_read_bytes:request.max_read_bytes,max_output_bytes:request.max_output_bytes,command_timeout_seconds:request.command_timeout_seconds,max_jobs:request.max_jobs,retained_jobs:request.retained_jobs,search_max_files:request.search_max_files,search_max_bytes:request.search_max_bytes};cfg.validate()?;let data=serde_json::to_value(&cfg.limits)?;let obj=data.as_object().context("Limits must be an object")?;let mut fields=Vec::new();for(key,value)in obj{let n=value.as_u64().context("Limits must use positive integers")?;let n=i64::try_from(n).context("Limits exceed TOML signed integer range")?;fields.push((key.as_str(),toml_edit::Value::from(n)));}edit_service_section(&mut text,"limits",&fields)?;let verify:Config=toml::from_str(&text).context("Generated Limits config is invalid")?;verify.validate()?;replace_config(path,text.as_bytes())?;Ok(json!({"revision":util::digest(text.as_bytes()),"requires_restart":true,"limits":verify.limits,"_operation_diff":config_diff(&original,&text)}))}
pub fn update_readonly_mounts(path:&Path,request:UpdateReadonlyMountsRequest)->Result<ServiceConfigMutation>{let(mut text,mut cfg)=read_checked(path,&request.expected_revision)?;let original=text.clone();cfg.execution.readonly_mounts=request.readonly_mounts.clone();cfg.validate()?;replace_execution_readonly_mounts(&mut text,&request.readonly_mounts)?;let verify:Config=toml::from_str(&text).context("Generated execution config is invalid")?;verify.validate()?;replace_config(path,text.as_bytes())?;Ok(ServiceConfigMutation{revision:util::digest(text.as_bytes()),requires_restart:true,execution:json!({"readonly_mounts":verify.execution.readonly_mounts}),operation_diff:config_diff(&original,&text)})}
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
    let project=ProjectConfig{id:id.into(),path:absolute.strip_prefix(&root)?.to_owned(),allow_write:request.allow_write,allow_exec:request.allow_exec,allow_git_commit:request.allow_git_commit,allow_git_mutation:request.allow_git_mutation,allow_git_push:request.allow_git_push,execution_profile:request.execution_profile.unwrap_or_else(config::default_project_profile),environment:request.environment};
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
            project.allow_write=request.allow_write;project.allow_exec=request.allow_exec;project.allow_git_commit=request.allow_git_commit;project.allow_git_mutation=request.allow_git_mutation;project.allow_git_push=request.allow_git_push;if let Some(profile)=request.execution_profile.clone(){project.execution_profile=profile;}if let Some(environment)=request.environment.clone(){project.environment=environment;}
        }
        let project=cfg.workspaces[workspace_index].projects[project_index].clone();let (start,end)=project_block_range(&text,workspace_index,project_index)?;let replacement=format!("[[workspaces.projects]]\n{}",toml::to_string(&project)?);text.replace_range(start..end,&replacement);
        project
    }else{
        let legacy_index=cfg.workspaces.iter().enumerate().find_map(|(index,w)|{
            if index==workspace_index||w.id!=project_id||!w.projects.is_empty()||!(w.allow_write.is_some()||w.allow_exec.is_some()||w.allow_git_commit.is_some()||w.allow_git_mutation.is_some()||w.allow_git_push.is_some()){return None;}
            let child=w.path.canonicalize().ok()?;if child==root||!child.starts_with(&root){return None;}Some(index)
        }).with_context(||format!("Project {workspace_id}/{project_id} is not configured"))?;
        let relative=cfg.workspaces[legacy_index].path.canonicalize()?.strip_prefix(&root)?.to_owned();
        {
            let legacy=&mut cfg.workspaces[legacy_index];legacy.allow_write=Some(request.allow_write);legacy.allow_exec=Some(request.allow_exec);legacy.allow_git_commit=Some(request.allow_git_commit);legacy.allow_git_mutation=Some(request.allow_git_mutation);legacy.allow_git_push=Some(request.allow_git_push);if let Some(profile)=request.execution_profile.clone(){legacy.execution_profile=Some(profile);}if let Some(environment)=request.environment.clone(){legacy.environment=environment;}
        }
        let legacy=cfg.workspaces[legacy_index].clone();let (start,end)=workspace_block_range(&text,legacy_index)?;let replacement=format!("[[workspaces]]\n{}",toml::to_string(&legacy)?);text.replace_range(start..end,&replacement);
        ProjectConfig{id:project_id.into(),path:relative,allow_write:request.allow_write,allow_exec:request.allow_exec,allow_git_commit:request.allow_git_commit,allow_git_mutation:request.allow_git_mutation,allow_git_push:request.allow_git_push,execution_profile:legacy.execution_profile.unwrap_or_else(config::default_project_profile),environment:legacy.environment}
    };
    cfg.validate()?;drop(workspace::load(&cfg,path)?);
    let verify:Config=toml::from_str(&text).context("Generated project config is invalid")?;verify.validate()?;drop(workspace::load(&verify,path)?);
    replace_config(path,text.as_bytes())?;
    Ok(ConfigMutation{revision:util::digest(text.as_bytes()),requires_restart:true,project:project_value(workspace_id,&project,&root),reload_warning:None,operation_diff:config_diff(&original,&text)})
}

#[cfg(test)]
mod tests{
    use super::*;
    fn config(root:&Path,state:&Path)->Config{let mut c=Config::default();c.security.data_dir=state.into();c.workspaces=vec![crate::config::WorkspaceConfig{id:"root".into(),path:root.into(),projects:vec![],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];c}

    #[test]
    fn add_project_still_rejects_relative_and_outside_workspace_paths(){
        let temp=tempfile::tempdir().unwrap();
        let root=temp.path().join("workspace");
        let child=root.join("inside");
        let outside=temp.path().join("outside");
        let state=temp.path().join("state");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let config_path=temp.path().join("config.toml");
        std::fs::write(&config_path,toml::to_string_pretty(&config(&root,&state)).unwrap()).unwrap();
        let mut request=AddProjectRequest{
            expected_revision:revision(&config_path).unwrap(),
            workspace:"root".into(),project:None,
            path:"inside".into(),allow_write:true,allow_exec:true,
            allow_git_commit:true,allow_git_mutation:false,
            allow_git_push:false,execution_profile:Some("development".into()),
            environment:vec![],
        };
        assert!(add_project(&config_path,request.clone()).unwrap_err().to_string().contains("absolute"));
        request.path=outside.to_string_lossy().into_owned();
        assert!(add_project(&config_path,request.clone()).unwrap_err().to_string().contains("strict descendant"));
        request.path=root.join("not-yet-created").to_string_lossy().into_owned();
        assert!(add_project(&config_path,request).is_err());
        assert!(Config::load_file(&config_path).unwrap().workspaces[0].projects.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn windows_add_project_accepts_drive_slashes_with_verbatim_workspace_root(){
        let temp=tempfile::tempdir().unwrap();
        let root=temp.path().join("workspace");
        let child=root.join("EndlessVibe");
        let state=temp.path().join("state");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        let canonical_root=root.canonicalize().unwrap();
        let root_text=canonical_root.to_string_lossy();
        assert!(root_text.starts_with(r"\\?\"),"Windows canonical path: {root_text}");
        let canonical_child=child.canonicalize().unwrap();
        let child_text=canonical_child.to_string_lossy();
        // From the OS's \\?\C:\... notation to a normal C:/... input.
        let friendly=child_text.strip_prefix(r"\\?\").unwrap_or(&child_text);
        let forward_slashes=friendly.replace('\\',"/");
        let config_path=temp.path().join("config.toml");
        std::fs::write(&config_path,toml::to_string_pretty(&config(&canonical_root,&state)).unwrap()).unwrap();
        let outcome=add_project(&config_path,AddProjectRequest{
            expected_revision:revision(&config_path).unwrap(),
            workspace:"root".into(),project:None,
            path:forward_slashes,allow_write:true,allow_exec:true,
            allow_git_commit:true,allow_git_mutation:false,
            allow_git_push:false,execution_profile:Some("development".into()),
            environment:vec![],
        }).unwrap();
        assert_eq!(outcome.project["id"],"EndlessVibe");
        let loaded=Config::load_file(&config_path).unwrap();
        assert_eq!(loaded.workspaces[0].projects[0].path,PathBuf::from("EndlessVibe"));
    }

    #[test]fn add_and_update_preserve_comments(){let t=tempfile::tempdir().unwrap();let root=t.path().join("root");let child=root.join("demo");let state=t.path().join("state");std::fs::create_dir_all(&child).unwrap();std::fs::create_dir_all(&state).unwrap();let path=t.path().join("config.toml");let mut text=toml::to_string_pretty(&config(&root,&state)).unwrap();text.push_str("\n# keep-comment\n");std::fs::write(&path,&text).unwrap();let rev=revision(&path).unwrap();let added=add_project(&path,AddProjectRequest{expected_revision:rev,workspace:"root".into(),project:None,path:child.display().to_string(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:Some("development".into()),environment:vec!["TEST_DATABASE_URL=postgres://localhost/test".into()]}).unwrap();assert_eq!(added.project["execution_profile"],"development");assert_eq!(added.project["environment_keys"],json!(["TEST_DATABASE_URL"]));assert!(!added.operation_diff.contains("postgres://localhost/test"));assert!(added.operation_diff.contains("environment = [REDACTED]"));let updated=update_project(&path,"root","demo",UpdateProjectRequest{expected_revision:added.revision,allow_write:true,allow_exec:false,allow_git_commit:false,allow_git_mutation:false,allow_git_push:false,execution_profile:Some("isolated".into()),environment:Some(vec![])}).unwrap();assert!(updated.requires_restart);let after=std::fs::read_to_string(&path).unwrap();assert!(after.contains("# keep-comment"));let loaded=Config::load_file(&path).unwrap();assert!(!loaded.workspaces[0].projects[0].allow_exec);}
    #[test]fn readonly_mounts_update_preserves_other_execution_text(){let t=tempfile::tempdir().unwrap();let root=t.path().join("root");let state=t.path().join("state");let sdk=t.path().join("android-sdk");let tools=t.path().join("tools");std::fs::create_dir_all(&root).unwrap();std::fs::create_dir_all(&state).unwrap();std::fs::create_dir_all(&sdk).unwrap();std::fs::create_dir_all(&tools).unwrap();let path=t.path().join("config.toml");let mut cfg=config(&root,&state);cfg.execution.readonly_mounts=vec![ReadOnlyMount{source:tools.clone(),target:"/opt/tools".into()}];let mut text=toml::to_string_pretty(&cfg).unwrap();text=text.replace("[execution]\n","[execution]\n# keep-execution-comment\n");std::fs::write(&path,&text).unwrap();let rev=revision(&path).unwrap();let result=update_readonly_mounts(&path,UpdateReadonlyMountsRequest{expected_revision:rev,readonly_mounts:vec![ReadOnlyMount{source:sdk.clone(),target:"/opt/android-sdk".into()},ReadOnlyMount{source:tools,target:"/opt/tools".into()}]}).unwrap();assert!(result.requires_restart);let after=std::fs::read_to_string(&path).unwrap();assert!(after.contains("# keep-execution-comment"));assert!(after.contains("/opt/android-sdk"));let loaded=Config::load_file(&path).unwrap();assert_eq!(loaded.execution.readonly_mounts.len(),2);assert_eq!(loaded.execution.readonly_mounts[0].target,PathBuf::from("/opt/android-sdk"));}
    #[test]fn online_git_and_limits_edits_preserve_comments_and_reject_stale_revisions(){let t=tempfile::tempdir().unwrap();let root=t.path().join("root");let state=t.path().join("state");std::fs::create_dir_all(&root).unwrap();std::fs::create_dir_all(&state).unwrap();let path=t.path().join("config.toml");let text=toml::to_string_pretty(&config(&root,&state)).unwrap().replace("[git]\n","[git]\n# keep-git-comment\n").replace("[limits]\n","[limits]\n# keep-limits-comment\n");std::fs::write(&path,text).unwrap();let original=revision(&path).unwrap();let git=update_git(&path,UpdateGitRequest{expected_revision:original.clone(),executable:"/usr/bin/git".into(),author_name:"Developer".into(),author_email:"developer@example.org".into()}).unwrap();assert!(git["requires_restart"].as_bool().unwrap());assert_eq!(git["git"]["author_name"],"Developer");assert!(std::fs::read_to_string(&path).unwrap().contains("# keep-git-comment"));assert!(update_git(&path,UpdateGitRequest{expected_revision:original,executable:"/usr/bin/git".into(),author_name:"Conflict".into(),author_email:"developer@example.org".into()}).is_err());let mut limit=Config::load_file(&path).unwrap().limits;limit.command_timeout_seconds=600;limit.retained_jobs=25;let out=update_limits(&path,UpdateLimitsRequest{expected_revision:git["revision"].as_str().unwrap().into(),max_file_bytes:limit.max_file_bytes,max_read_bytes:limit.max_read_bytes,max_output_bytes:limit.max_output_bytes,command_timeout_seconds:limit.command_timeout_seconds,max_jobs:limit.max_jobs,retained_jobs:limit.retained_jobs,search_max_files:limit.search_max_files,search_max_bytes:limit.search_max_bytes}).unwrap();assert_eq!(out["limits"]["command_timeout_seconds"],600);assert_eq!(out["limits"]["retained_jobs"],25);assert!(std::fs::read_to_string(&path).unwrap().contains("# keep-limits-comment"));let mut bad=UpdateLimitsRequest{expected_revision:out["revision"].as_str().unwrap().into(),max_file_bytes:limit.max_file_bytes,max_read_bytes:limit.max_read_bytes,max_output_bytes:limit.max_output_bytes,command_timeout_seconds:3601,max_jobs:limit.max_jobs,retained_jobs:limit.retained_jobs,search_max_files:limit.search_max_files,search_max_bytes:limit.search_max_bytes};assert!(update_limits(&path,bad.clone()).is_err());bad.command_timeout_seconds=600;bad.max_read_bytes=bad.max_file_bytes+1;assert!(update_limits(&path,bad).is_err());assert_eq!(Config::load_file(&path).unwrap().limits.command_timeout_seconds,600);}
    #[test]fn service_sections_can_be_created_when_missing(){let t=tempfile::tempdir().unwrap();let path=t.path().join("config.toml");std::fs::write(&path,format!("# minimal config\n[security]\ndata_dir = {:?}\n",t.path().join("state").display().to_string())).unwrap();let rev=revision(&path).unwrap();let git=update_git(&path,UpdateGitRequest{expected_revision:rev,executable:"/usr/bin/git".into(),author_name:"Minimal".into(),author_email:"minimal@example.org".into()}).unwrap();let limits=Config::load_file(&path).unwrap().limits;let value=update_limits(&path,UpdateLimitsRequest{expected_revision:git["revision"].as_str().unwrap().into(),max_file_bytes:limits.max_file_bytes,max_read_bytes:limits.max_read_bytes,max_output_bytes:limits.max_output_bytes,command_timeout_seconds:limits.command_timeout_seconds,max_jobs:limits.max_jobs,retained_jobs:limits.retained_jobs,search_max_files:limits.search_max_files,search_max_bytes:limits.search_max_bytes}).unwrap();assert_eq!(value["requires_restart"],true);let text=std::fs::read_to_string(&path).unwrap();assert!(text.contains("# minimal config"));assert!(text.contains("[git]"));assert!(text.contains("[limits]"));}
    #[test]fn transfer_config_online_edit_preserves_comments_and_validates(){let t=tempfile::tempdir().unwrap();let path=t.path().join("config.toml");let text=toml::to_string_pretty(&Config::default()).unwrap().replace("[transfer]\n","[transfer]\n# keep-transfer-comment\n");std::fs::write(&path,text).unwrap();let rev=revision(&path).unwrap();let cfg=UpdateTransferRequest{expected_revision:rev.clone(),enabled:true,listen:"0.0.0.0:20002".into(),advertise:true,discover:true,display_name:"Child".into()};let result=update_transfer(&path,cfg.clone()).unwrap();assert_eq!(result["requires_restart"],true);assert_eq!(Config::load_file(&path).unwrap().transfer.display_name,"Child");assert!(std::fs::read_to_string(&path).unwrap().contains("# keep-transfer-comment"));assert!(update_transfer(&path,cfg).is_err());}
    #[cfg(unix)]
    #[test]fn docker_config_edit_requires_explicit_acknowledgement_and_preserves_comments(){let t=tempfile::tempdir().unwrap();let path=t.path().join("config.toml");let text=toml::to_string_pretty(&Config::default()).unwrap().replace("[docker]\n","[docker]\n# keep-docker-comment\n");std::fs::write(&path,text).unwrap();let rev=revision(&path).unwrap();let mut request=UpdateDockerRequest{expected_revision:rev.clone(),enabled:true,socket:"/var/run/docker.sock".into(),allowed_containers:vec!["endlessvibe".into()],allow_start:false,allow_stop:false,allow_restart:true,allow_project_socket:false,acknowledge_daemon_control:false};assert!(update_docker(&path,request.clone()).is_err());request.acknowledge_daemon_control=true;let v=update_docker(&path,request.clone()).unwrap();assert_eq!(v["requires_restart"],true);assert_eq!(v["docker"]["allowed_containers"],json!(["endlessvibe"]));assert!(std::fs::read_to_string(&path).unwrap().contains("# keep-docker-comment"));assert!(update_docker(&path,request.clone()).is_err());let new_rev=v["revision"].as_str().unwrap().to_owned();request.expected_revision=new_rev;request.allowed_containers.push("../../docker".into());assert!(update_docker(&path,request).is_err());let cfg=Config::load_file(&path).unwrap();assert_eq!(cfg.docker.allowed_containers,vec!["endlessvibe"]);assert!(!cfg.docker.allow_project_socket);}
    #[test]fn legacy_child_project_permissions_are_editable(){let t=tempfile::tempdir().unwrap();let root=t.path().join("projects");let child=root.join("EndlessVibe");let state=t.path().join("state");std::fs::create_dir_all(&child).unwrap();std::fs::create_dir_all(&state).unwrap();let path=t.path().join("config.toml");let mut cfg=Config::default();cfg.security.data_dir=state;cfg.workspaces=vec![crate::config::WorkspaceConfig{id:"NAME".into(),path:root.clone(),projects:vec![],allow_write:Some(true),allow_exec:Some(true),allow_git_commit:Some(true),allow_git_mutation:Some(false),allow_git_push:Some(false),execution_profile:None,environment:vec![]},crate::config::WorkspaceConfig{id:"EndlessVibe".into(),path:child,projects:vec![],allow_write:Some(true),allow_exec:Some(true),allow_git_commit:Some(true),allow_git_mutation:Some(false),allow_git_push:Some(false),execution_profile:None,environment:vec![]}];std::fs::write(&path,toml::to_string_pretty(&cfg).unwrap()).unwrap();let rev=revision(&path).unwrap();update_project(&path,"NAME","EndlessVibe",UpdateProjectRequest{expected_revision:rev,allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:true,allow_git_push:true,execution_profile:Some("development".into()),environment:Some(vec!["LEGACY_SERVICE=http://127.0.0.1:19031".into()])}).unwrap();let loaded=Config::load_file(&path).unwrap();let legacy=loaded.workspaces.iter().find(|w|w.id=="EndlessVibe").unwrap();assert_eq!(legacy.allow_git_mutation,Some(true));assert_eq!(legacy.allow_git_push,Some(true));assert_eq!(legacy.execution_profile.as_deref(),Some("development"));assert_eq!(legacy.environment,vec!["LEGACY_SERVICE=http://127.0.0.1:19031"]);let effective=workspace::load(&loaded,&path).unwrap();assert!(effective["NAME"].projects["EndlessVibe"].config.allow_git_mutation);assert!(effective["NAME"].projects["EndlessVibe"].config.allow_git_push);assert!(effective["NAME"].projects["EndlessVibe"].config.development());assert_eq!(effective["NAME"].projects["EndlessVibe"].config.environment_map().unwrap()["LEGACY_SERVICE"],"http://127.0.0.1:19031");}
}
