//! Compatibility routing for clients that cached the original 20 one-project MCP tools.
//!
//! A virtual workspace node:<24-hex-id>:<workspace-id>:<project-id> is an
//! explicit child selector, NOT a local filesystem path or a new grant.
//! Every call still uses pinned-TLS Transfer and is re-authorized by the child.
use super::{answer,answer_logged};
use crate::{config, runtime::Runtime, transfer::router, util};
use anyhow::{bail, Result};
use rmcp::model::CallToolResult;
use serde::Serialize;
use serde_json::{json,Value};
use std::{sync::Arc,time::Duration};

const PREFIX:&str="node:";
const DISCOVERY_TIMEOUT:Duration=Duration::from_secs(4);

#[derive(Debug,Clone,PartialEq,Eq)]
pub(super) struct Target{pub node_id:String,pub workspace:String,pub project:String}
impl Target{
    pub fn alias(&self)->String{format!("{PREFIX}{}:{}:{}",self.node_id,self.workspace,self.project)}
}
pub(super) fn parse(workspace:&str,project:&str)->Result<Option<Target>>{
    if !workspace.starts_with(PREFIX){return Ok(None);}
    if !project.is_empty(){bail!("Virtual node selector must occupy the legacy workspace field; omit project");}
    let fields=workspace.split(':').collect::<Vec<_>>();
    if fields.len()!=4||fields[0]!="node"||fields[1].len()!=24
      ||!fields[1].bytes().all(|c|c.is_ascii_hexdigit())
      ||!config::valid_id(fields[2])||!config::valid_id(fields[3]){
        bail!("Invalid remote Project selector; expected node:<24-hex-node-id>:<workspace>:<project>");
    }
    Ok(Some(Target{node_id:fields[1].into(),workspace:fields[2].into(),project:fields[3].into()}))
}
fn canonical_args<T:Serialize>(a:&T,target:&Target)->Result<Value>{
    let mut args=serde_json::to_value(a)?;
    args["workspace"]=json!(target.workspace);
    args["project"]=json!(target.project);
    Ok(args)
}

/// Dispatch only existing read-only tools; cannot turn arbitrary tool names into Transfer RPCs.
/// None means the caller provided an ordinary local Workspace/Project address.
pub(super) async fn forward_read<T:Serialize>(rt:&Arc<Runtime>,tool:&'static str,
    workspace:&str,project:&str,a:&T)->Option<CallToolResult>{
    let target=match parse(workspace,project){
        Ok(Some(target))=>target,
        Ok(None)=>return None,
        Err(error)=>return Some(answer(Err(error))),
    };
    if !router::readonly(tool){return Some(answer(Err(anyhow::anyhow!("Remote tool is not approved for read-only routing"))));}
    let args=match canonical_args(a,&target){Ok(args)=>args,Err(error)=>return Some(answer(Err(error)))};
    let op=rt.begin_operation("node_read",&target.workspace,&target.project,
        json!({"node_id":target.node_id,"tool":tool,"arguments_sha256":util::digest(args.to_string())}));
    Some(answer_logged(rt,op,rt.transfer.call_node(&target.node_id,tool,args).await))
}

/// Preserve the legacy local project list, appending only projects that the
/// paired child itself reports as read-authorized. Offline peers remain visible
/// as status entries without guessing grants or filesystem paths.
pub(super) async fn projects(rt:&Arc<Runtime>,workspace:&str)->Result<Value>{
    let mut value=rt.projects_for(workspace)?;
    if !workspace.is_empty()||!rt.transfer.config.enabled{return Ok(value);}
    let peers=rt.transfer.peers()?;
    let mut remote_nodes=Vec::new();
    for peer in peers["peers"].as_array().into_iter().flatten()
        .filter(|p|p["role"]=="parent").take(8){
        let Some(node_id)=peer["node_id"].as_str()else{continue};
        if node_id.len()!=24||!node_id.bytes().all(|c|c.is_ascii_hexdigit()){continue;}
        let name=peer["name"].as_str().unwrap_or("Child");
        let read_workspaces=tokio::time::timeout(DISCOVERY_TIMEOUT,
            rt.transfer.call_node(node_id,"list_workspaces",json!({}))).await;
        let workspaces=match read_workspaces{
            Ok(Ok(result))=>result,
            Ok(Err(_))|Err(_)=>{
                remote_nodes.push(json!({"node_id":node_id,"name":name,"status":"unreachable_or_unauthorized"}));
                continue;
            }
        };
        let mut count=0usize;
        for item in workspaces["workspaces"].as_array().into_iter().flatten().take(16){
            let Some(w)=item["id"].as_str().filter(|w|config::valid_id(w))else{continue};
            let read_projects=tokio::time::timeout(DISCOVERY_TIMEOUT,
                rt.transfer.call_node(node_id,"list_projects",json!({"workspace":w}))).await;
            let Ok(Ok(list))=read_projects else{continue};
            for project in list["projects"].as_array().into_iter().flatten().take(32){
                let Some(p)=project["id"].as_str().filter(|p|config::valid_id(p))else{continue};
                let target=Target{node_id:node_id.into(),workspace:w.into(),project:p.into()};
                let permissions=&project["permissions"];
                value["projects"].as_array_mut().expect("Local projects are an array").push(json!({
                    "workspace":format!("node:{node_id}"),
                    "id":target.alias(),
                    "path":format!("paired child {name} / {w} / {p}"),
                    "remote":true,"node_id":node_id,"remote_workspace":w,"remote_project":p,
                    "allow_write":permissions["write"]==true,
                    "allow_exec":permissions["execute"]==true,
                    "allow_git_commit":permissions["git"]==true,
                    "allow_git_mutation":false,"allow_git_push":false,
                    "execution_profile":"remote_transfer",
                }));
                count+=1;
            }
        }
        remote_nodes.push(json!({"node_id":node_id,"name":name,"status":"paired",
            "visible_read_projects":count}));
    }
    value["remote_nodes"]=json!(remote_nodes);
    value["remote_addressing"]=json!("Use the returned node:... project id as workspace in cached legacy tools; child grants and pinned TLS are checked on every request.");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn virtual_selectors_are_explicit_and_strict(){
        assert!(parse("EndlessVibe","").unwrap().is_none());
        let remote=Target{node_id:"0123456789abcdef01234567".into(),workspace:"NAME".into(),project:"EndlessVibe".into()};
        assert_eq!(parse(&remote.alias(),"").unwrap(),Some(remote.clone()));
        for invalid in [
            "node:0123456789abcdef01234567:NAME",
            "node:0123456789abcdef01234567:NAME:../escape",
            "node:0123456789abcdef01234567:NAME:foo:bar",
            "node:0123456789abcdef01234567:NAME:%00",
            "node:a:NAME:EndlessVibe",
        ]{assert!(parse(invalid,"").is_err(),"{invalid}");}
        assert!(parse(&remote.alias(),"something").is_err());
    }
}
