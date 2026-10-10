use super::{secure::{Peer,Wire},TransferManager};
use crate::{runtime::Runtime,tools::{filesystem,git,types::*},util};
use anyhow::{bail,Context,Result};
use serde::{de::DeserializeOwned,Deserialize,Serialize};
use schemars::JsonSchema;
use serde_json::{json,Value};
use std::sync::Arc;

#[derive(Clone,Debug,Serialize,Deserialize,JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeCallArgs{pub node_id:String,pub tool:String,#[serde(default)]pub arguments:Value}
pub fn readonly(tool:&str)->bool{matches!(tool,"list_workspaces"|"list_projects"|"inspect_project"|"list_directory"|"read_file"|"search_code"|"git_status"|"git_diff"|"git_log")}
fn args<T:DeserializeOwned>(v:&Value)->Result<T>{serde_json::from_value(v.clone()).context("Invalid remote tool parameters")}
fn target(v:&Value)->Result<(String,String)>{let w=v.get("workspace").and_then(Value::as_str).context("Remote request requires an exact workspace")?;let p=v.get("project").and_then(Value::as_str).context("Remote request requires an exact project")?;if w.is_empty()||p.is_empty(){bail!("Remote Project names cannot be empty");}Ok((w.into(),p.into()))}
fn read_grant<'a>(peer:&'a Peer,w:&str,p:&str)->Result<&'a super::secure::Grant>{peer.grants.iter().find(|g|g.workspace==w&&g.project==p&&g.read).context("Remote read access was not granted for this Project")}
pub async fn dispatch(rt:Arc<Runtime>,transfer:Arc<TransferManager>,wire:Wire)->Result<Value>{
 if wire.kind!="call"||wire.tool.len()>64||wire.tool.is_empty(){bail!("Invalid Transfer RPC");}
 let peer=transfer.incoming_peer(&wire.node_id,&wire.token)?;
 let tool=wire.tool.as_str();if !readonly(tool){bail!("Remote mutation tools are not enabled");}let input=wire.args.clone();
 let(w,p)=if matches!(tool,"list_workspaces"|"list_projects"){(String::new(),String::new())}else{target(&input)?};
 let operation=rt.begin_operation(&format!("transfer_{tool}"),&w,&p,json!({"parent_node":wire.node_id,"arguments":input}));
 let result=dispatch_inner(rt.clone(),peer,tool,input,&w,&p).await;
 rt.finish_operation(operation,result)
}
async fn dispatch_inner(rt:Arc<Runtime>,peer:Peer,tool:&str,v:Value,w:&str,p:&str)->Result<Value>{
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
 _=>bail!("Remote Transfer tool is not enabled in this stage")
 }
}
#[cfg(test)]mod tests{
 use super::*;#[test]fn remote_routes_require_exact_names(){assert!(target(&json!({"workspace":"w","project":"p"})).is_ok());assert!(target(&json!({"workspace":"w"})).is_err());assert!(target(&json!({"workspace":"w","project":""})).is_err());}
}
