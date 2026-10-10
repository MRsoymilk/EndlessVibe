use super::{secure::{Peer,Wire},TransferManager};
use crate::{runtime::Runtime,tools::{filesystem,git,tasks,types::*},util};
use anyhow::{bail,Context,Result};
use serde::{de::DeserializeOwned,Deserialize,Serialize};
use schemars::JsonSchema;
use serde_json::{json,Value};
use std::sync::Arc;

#[derive(Clone,Debug,Serialize,Deserialize,JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeCallArgs{pub node_id:String,pub tool:String,#[serde(default)]pub arguments:Value,#[serde(default)]pub request_id:Option<String>}
pub fn readonly(tool:&str)->bool{matches!(tool,"list_workspaces"|"list_projects"|"inspect_project"|"list_directory"|"read_file"|"search_code"|"git_status"|"git_diff"|"git_log"|"get_task_checkpoint"|"continue_task"|"list_task_checkpoints"|"request_status"|"request_history")}
pub fn writable(tool:&str)->bool{matches!(tool,"write_file"|"apply_patch"|"create_directory"|"run_command"|"get_job"|"get_job_output"|"cancel_job"|"git_commit")}
fn permission(tool:&str)->Option<&'static str>{match tool{"write_file"|"apply_patch"|"create_directory"=>Some("write"),"run_command"|"get_job"|"get_job_output"|"cancel_job"=>Some("execute"),"git_commit"=>Some("git"),t if readonly(t)=>Some("read"),_=>None}}
fn args<T:DeserializeOwned>(v:&Value)->Result<T>{serde_json::from_value(v.clone()).context("Invalid remote tool parameters")}
fn target(v:&Value)->Result<(String,String)>{let w=v.get("workspace").and_then(Value::as_str).context("Remote request requires an exact workspace")?;let p=v.get("project").and_then(Value::as_str).context("Remote request requires an exact project")?;if w.is_empty()||p.is_empty(){bail!("Remote Project names cannot be empty");}Ok((w.into(),p.into()))}
fn read_grant<'a>(peer:&'a Peer,w:&str,p:&str)->Result<&'a super::secure::Grant>{peer.grants.iter().find(|g|g.workspace==w&&g.project==p&&g.read).context("Remote read access was not granted for this Project")}

fn enrich_job_status(rt:&Runtime,row:&mut Value,workspace:&str,project:&str){
 let Some(job_id)=row.get("job_id").and_then(Value::as_str).map(str::to_owned)else{return;};
 match rt.jobs.get(&job_id){
  Ok(job) if job["workspace"]==workspace&&job["project"]==project=>{
   row["job_status"]=job["status"].clone();
   row["recovery_action"]=json!(if matches!(job["status"].as_str(),Some("queued"|"running")){"poll_job"}else{"inspect_job"});
  }
  _=>{
   row["job_status"]=json!("unavailable");
   row["recovery_action"]=json!("inspect_project_before_new_request");
  }
 }
}

pub async fn dispatch(rt:Arc<Runtime>,transfer:Arc<TransferManager>,wire:Wire)->Result<Value>{
 if wire.kind!="call"||wire.tool.len()>64||wire.tool.is_empty(){bail!("Invalid Transfer RPC");}
 let peer=transfer.incoming_peer(&wire.node_id,&wire.token)?;
 let tool=wire.tool.as_str();if permission(tool).is_none(){bail!("Transfer tool is not authorized");}let input=wire.args.clone();
 let(w,p)=if matches!(tool,"list_workspaces"|"list_projects"){(String::new(),String::new())}else{target(&input)?};
 if writable(tool)&&!matches!(tool,"get_job"|"get_job_output"){
  // Authorization must precede both deduplication and cached result delivery.
  let grant=peer.grants.iter().find(|g|g.workspace==w&&g.project==p).context("Remote Project not granted")?;
  let local=rt.project_exact(&w,&p)?;
  let permitted=match permission(tool){Some("write")=>grant.write&&local.config.allow_write,Some("execute")=>grant.execute&&local.config.allow_exec&&local.config.allow_write,Some("git")=>grant.git&&local.config.allow_git_commit&&local.config.allow_write,_=>false};
  if !permitted{bail!("Remote operation exceeds current Project permission");}
  match super::idempotency::claim(&rt.db,&wire.node_id,&wire.id,tool,&input)?{super::idempotency::Claim::Cached(mut result)=>{if tool=="run_command"{result["reused"]=json!(true);}return Ok(result)},super::idempotency::Claim::New=>{}}
 }
 let summary=if matches!(tool,"write_file"|"apply_patch"){json!({"parent_node":wire.node_id,"tool":tool,"workspace":w,"project":p,"payload_sha256":util::digest(input.to_string())})}else{json!({"parent_node":wire.node_id,"arguments":input})};let operation=rt.begin_operation(&format!("transfer_{tool}"),&w,&p,summary);
 let result=dispatch_inner(rt.clone(),peer,tool,input,&w,&p).await;
 let result=rt.finish_operation(operation,result);
 if writable(tool)&&!matches!(tool,"get_job"|"get_job_output"){
  match &result{Ok(value)=>super::idempotency::complete(&rt.db,&wire.node_id,&wire.id,value)?,Err(_)=>super::idempotency::fail(&rt.db,&wire.node_id,&wire.id)?}
 }
 result
}
async fn dispatch_inner(rt:Arc<Runtime>,peer:Peer,tool:&str,v:Value,w:&str,p:&str)->Result<Value>{
 if let Some(right)=permission(tool){if !matches!(tool,"list_workspaces"|"list_projects"){let grant=peer.grants.iter().find(|g|g.workspace==w&&g.project==p).context("Remote Project not granted")?;let local=rt.project_exact(w,p)?;let allowed=match right{"read"=>grant.read,"write"=>grant.write&&local.config.allow_write,"execute"=>grant.execute&&local.config.allow_exec&&local.config.allow_write,"git"=>grant.git&&local.config.allow_git_commit&&local.config.allow_write,_=>false};if !allowed{bail!("Remote operation exceeds child Project grant or current local permissions");}}}
 match tool{
 "list_workspaces"=>{let mut ids=peer.grants.iter().filter(|g|g.read).map(|g|g.workspace.clone()).collect::<Vec<_>>();ids.sort();ids.dedup();Ok(json!({"workspaces":ids.iter().map(|id|json!({"id":id})).collect::<Vec<_>>(),"node_id":rt.transfer.node_id}))}
 "list_projects"=>{let workspace=v.get("workspace").and_then(Value::as_str).context("workspace is required")?;let projects=peer.grants.iter().filter(|g|g.workspace==workspace&&g.read).filter_map(|g|rt.project_exact(workspace,&g.project).ok().map(|_|json!({"workspace":workspace,"id":g.project,"permissions":{"read":g.read,"write":g.write,"execute":g.execute,"git":g.git}}))).collect::<Vec<_>>();Ok(json!({"workspace":workspace,"projects":projects}))}
 "inspect_project"=>{read_grant(&peer,w,p)?;let project=rt.project_exact(w,p)?;rt.sync_project("transfer_inspect",w,p,move|_,p|crate::workspace::inspect(&p)).await.map(|mut v|{v["node_id"]=json!(rt.transfer.node_id);v})}
 "list_directory"=>{read_grant(&peer,w,p)?;let a:DirectoryArgs=args(&v)?;rt.project_exact(w,p)?;rt.sync_project("transfer_list_directory",w,p,move|_,project|filesystem::list(&project,a)).await}
 "read_file"=>{read_grant(&peer,w,p)?;let a:ReadArgs=args(&v)?;rt.project_exact(w,p)?;rt.sync_project("transfer_read_file",w,p,move|rt,project|filesystem::read(&rt,&project,a)).await}
 "search_code"=>{read_grant(&peer,w,p)?;let a:SearchArgs=args(&v)?;rt.project_exact(w,p)?;rt.sync_project("transfer_search_code",w,p,move|rt,project|filesystem::search(&rt,&project,a)).await}
 "git_status"=>{read_grant(&peer,w,p)?;rt.project_exact(w,p)?;rt.asynchronous_project("transfer_git_status",w,p,move|rt,project|async move{git::status(&rt,&project).await}).await}
 "git_diff"=>{read_grant(&peer,w,p)?;let a:DiffArgs=args(&v)?;rt.project_exact(w,p)?;rt.asynchronous_project("transfer_git_diff",w,p,move|rt,project|async move{git::diff(&rt,&project,a).await}).await}
 "git_log"=>{read_grant(&peer,w,p)?;let a:LogArgs=args(&v)?;rt.project_exact(w,p)?;rt.asynchronous_project("transfer_git_log",w,p,move|rt,project|async move{git::log(&rt,&project,a).await}).await}
 "get_task_checkpoint"=>{let a:TaskArgs=args(&v)?;if a.workspace!=w||a.project!=p{bail!("Remote checkpoint request must use exact authorized Project");}tasks::get(&rt.db,a)}
 "continue_task"=>{let a:ContinueTaskArgs=args(&v)?;if a.workspace!=w||a.project!=p{bail!("Remote recovery request must use exact authorized Project");}tasks::continue_recovery(&rt.db,a)}
 "request_status"=>{let id=v.get("request_id").and_then(Value::as_str).context("request_id is required")?;let mut state=super::idempotency::status(&rt.db,&peer.node_id,id,w,p)?;enrich_job_status(&rt,&mut state,w,p);Ok(state)}
 "request_history"=>{let limit=v.get("limit").and_then(Value::as_u64).unwrap_or(20);if !(1..=50).contains(&limit){bail!("Transfer history limit must be 1..50");}let mut history=super::idempotency::history(&rt.db,&peer.node_id,w,p,limit as usize)?;if let Some(entries)=history["requests"].as_array_mut(){for entry in entries{enrich_job_status(&rt,entry,w,p);}}Ok(history)}
 "list_task_checkpoints"=>{let query:ListTaskCheckpointsArgs=args(&v)?;if query.workspace.as_deref()!=Some(w)||query.project.as_deref()!=Some(p){bail!("Remote checkpoint query must use exact authorized Project");}tasks::list(&rt.db,query)}
 "write_file"=>{let a:WriteArgs=args(&v)?;rt.sync_project("transfer_write_file",w,p,move|rt,project|filesystem::write(&rt,&project,a)).await}
 "apply_patch"=>{let a:PatchArgs=args(&v)?;rt.sync_project("transfer_apply_patch",w,p,move|rt,project|filesystem::patch(&rt,&project,a)).await}
 "create_directory"=>{let a:MakeDirectoryArgs=args(&v)?;rt.sync_project("transfer_create_directory",w,p,move|_,project|filesystem::mkdir(&project,a)).await}
 "run_command"=>{let a:CommandArgs=args(&v)?;rt.jobs.submit(rt.clone(),a,false).await}
 "get_job"=>{let a:JobArgs=args(&json!({"job_id":v["job_id"]}))?;let job=rt.jobs.get(&a.job_id)?;if job["workspace"]!=w||job["project"]!=p{bail!("Job does not belong to authorized Project");}Ok(job)}
 "get_job_output"=>{let a:OutputArgs=args(&json!({"job_id":v["job_id"],"offset":v.get("offset").cloned().unwrap_or(json!(0)),"limit":v.get("limit").cloned().unwrap_or(json!(8192))}))?;let job=rt.jobs.get(&a.job_id)?;if job["workspace"]!=w||job["project"]!=p{bail!("Job does not belong to authorized Project");}rt.jobs.output(a)}
 "cancel_job"=>{let a:JobArgs=args(&json!({"job_id":v["job_id"]}))?;let job=rt.jobs.get(&a.job_id)?;if job["workspace"]!=w||job["project"]!=p{bail!("Job does not belong to authorized Project");}rt.jobs.cancel(&a.job_id)}
 "git_commit"=>{let a:CommitArgs=args(&v)?;rt.asynchronous_project("transfer_git_commit",w,p,move|rt,project|async move{git::commit(&rt,&project,a).await}).await}
 _=>bail!("Remote Transfer tool is not enabled")
 }
}
#[cfg(test)]mod tests{
 use super::*;#[test]fn mutation_tool_allowlist_excludes_shell_and_push(){for tool in ["write_file","apply_patch","create_directory","run_command","git_commit","get_job","get_job_output","cancel_job"]{assert!(writable(tool));}for tool in ["run_shell","git_push","docker_restart","list_workspaces","read_file"]{assert!(!writable(tool));}assert_eq!(permission("git_commit"),Some("git"));}
 #[test]fn remote_routes_require_exact_names(){assert!(target(&json!({"workspace":"w","project":"p"})).is_ok());assert!(target(&json!({"workspace":"w"})).is_err());assert!(target(&json!({"workspace":"w","project":""})).is_err());}
}
