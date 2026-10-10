use crate::{config_edit::{AddProjectRequest,UpdateProjectRequest,UpdateReadonlyMountsRequest,UpdateGitRequest,UpdateLimitsRequest,UpdateDockerRequest,UpdateTransferRequest},runtime::Runtime};
use axum::{extract::{Path as AxumPath,Query,State},http::{header,StatusCode},response::{sse::{Event,KeepAlive,Sse},Html,IntoResponse,Response},Json};
use serde::Deserialize;
use serde_json::{json,Value};
use std::{convert::Infallible,sync::Arc,time::Duration};
use tokio_stream::{wrappers::BroadcastStream,StreamExt};

pub async fn home()->impl IntoResponse{Html(include_str!("../web/index.html"))}
pub async fn css()->impl IntoResponse{([(header::CONTENT_TYPE,"text/css; charset=utf-8")],include_str!("../web/app.css"))}
pub async fn javascript()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/app.js"))}
pub async fn javascript_common()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/js/common.js"))}
pub async fn javascript_nodes()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/js/nodes.js"))}
pub async fn javascript_mcp()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/js/mcp.js"))}
pub async fn javascript_charts()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/js/charts.js"))}
pub async fn uplot_javascript()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/vendor/uPlot/uPlot.iife.min.js"))}
pub async fn uplot_css()->impl IntoResponse{([(header::CONTENT_TYPE,"text/css; charset=utf-8")],include_str!("../web/vendor/uPlot/uPlot.min.css"))}
pub async fn favicon()->impl IntoResponse{StatusCode::NO_CONTENT}
pub async fn status(State(rt):State<Arc<Runtime>>)->impl IntoResponse{Json(rt.snapshot())}
pub async fn metrics(State(rt):State<Arc<Runtime>>)->impl IntoResponse{match rt.db.dashboard_metrics(3600,60){Ok(value)=>Json(value).into_response(),Err(error)=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("{error:#}")}))).into_response()}}
pub async fn activity(State(rt):State<Arc<Runtime>>)->impl IntoResponse{let audits=rt.db.audits(50);let jobs=rt.jobs.list(crate::tools::types::ListJobsArgs{workspace:None,project:None,task_id:None,limit:30});match(audits,jobs){(Ok(audits),Ok(jobs))=>Json(json!({"generated_at":crate::util::now(),"audits":audits,"jobs":jobs["jobs"]})).into_response(),(Err(error),_)|(_,Err(error))=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("{error:#}")}))).into_response()}}
pub async fn node_discoveries(State(rt):State<Arc<Runtime>>)->impl IntoResponse{Json(rt.transfer.discoveries())}
pub async fn node_read(State(rt):State<Arc<Runtime>>,Json(args):Json<crate::transfer::router::NodeCallArgs>)->Response{if !crate::transfer::router::readonly(&args.tool){return (StatusCode::FORBIDDEN,Json(json!({"error":"Only read-only tools are allowed"}))).into_response();}mutation_response(rt.transfer.call_node(&args.node_id,&args.tool,args.arguments).await)}
#[derive(Deserialize)]#[serde(deny_unknown_fields)]pub struct NodeWriteDashboard{pub node_id:String,pub tool:String,pub arguments:Value,pub confirm:bool}
pub async fn node_write(State(rt):State<Arc<Runtime>>,Json(args):Json<NodeWriteDashboard>)->Response{if !crate::transfer::router::writable(&args.tool)||!args.confirm{return (StatusCode::FORBIDDEN,Json(json!({"error":"Explicit confirmation and an authorized remote operation are required"}))).into_response();}let audit=json!({"node_id":args.node_id,"tool":args.tool,"arguments_sha256":crate::util::digest(args.arguments.to_string())});let operation=rt.begin_operation("node_dashboard_write","","",audit);let result=rt.transfer.call_node(&args.node_id,&args.tool,args.arguments).await;mutation_response(rt.finish_operation(operation,result))}
pub async fn node_pending(State(rt):State<Arc<Runtime>>)->Response{mutation_response(rt.transfer.pending())}
pub async fn node_peers(State(rt):State<Arc<Runtime>>)->Response{mutation_response(rt.transfer.peers())}
pub async fn node_pair_start(State(rt):State<Arc<Runtime>>,Json(request):Json<crate::transfer::secure::PairStart>)->Response{mutation_response(rt.transfer.start_pair(request.address).await)}
pub async fn node_pair_approve(State(rt):State<Arc<Runtime>>,Json(request):Json<crate::transfer::secure::PairApprove>)->Response{mutation_response(rt.transfer.approve_pair(request,&rt))}
pub async fn node_pair_revoke(State(rt):State<Arc<Runtime>>,AxumPath(node_id):AxumPath<String>)->Response{mutation_response(rt.transfer.revoke_pair(&node_id))}
pub async fn config(State(rt):State<Arc<Runtime>>)->Response{match rt.dashboard_config(){Ok(value)=>Json(value).into_response(),Err(error)=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("{error:#}")}))).into_response()}}
pub async fn project_states(State(rt):State<Arc<Runtime>>)->Response{let mut projects=Vec::new();for(workspace_id,project)in rt.projects_snapshot(){let base=json!({"workspace":workspace_id.clone(),"project":project.config.id});let value=match project.lock.clone().try_lock_owned(){Ok(_guard)=>match crate::tools::git::status(&rt,&project).await{Ok(status)=>{let changes=status["entries"].as_array().map(|v|v.len()).unwrap_or(0);json!({"workspace":workspace_id,"project":project.config.id,"state":"ready","branch":status["branch"],"head":status["head"],"changes":changes,"dirty":changes>0})},Err(error)=>{let message=format!("{error:#}");let state=if message.contains("not a standalone Git repository")||message.contains(".git must be a real directory"){"not_repository"}else{"unavailable"};let mut value=base;value["state"]=json!(state);value}},Err(_)=>{let mut value=base;value["state"]=json!("busy");value}};projects.push(value);}Json(json!({"generated_at":crate::util::now(),"projects":projects})).into_response()}
pub async fn reload_config(State(rt):State<Arc<Runtime>>)->Response{mutation_response(rt.dashboard_reload_config())}
pub async fn update_readonly_mounts(State(rt):State<Arc<Runtime>>,Json(request):Json<UpdateReadonlyMountsRequest>)->Response{mutation_response(rt.dashboard_update_readonly_mounts(request))}
pub async fn update_transfer(State(rt):State<Arc<Runtime>>,Json(request):Json<UpdateTransferRequest>)->Response{mutation_response(rt.dashboard_update_transfer(request))}
pub async fn update_docker(State(rt):State<Arc<Runtime>>,Json(request):Json<UpdateDockerRequest>)->Response{mutation_response(rt.dashboard_update_docker(request))}
pub async fn update_git(State(rt):State<Arc<Runtime>>,Json(request):Json<UpdateGitRequest>)->Response{mutation_response(rt.dashboard_update_git(request))}
pub async fn update_limits(State(rt):State<Arc<Runtime>>,Json(request):Json<UpdateLimitsRequest>)->Response{mutation_response(rt.dashboard_update_limits(request))}
pub async fn storage_maintenance(State(rt):State<Arc<Runtime>>)->Response{mutation_response(rt.dashboard_storage_maintenance())}
pub async fn storage_cache_cleanup(State(rt):State<Arc<Runtime>>,Json(request):Json<crate::storage::CleanupRequest>)->Response{mutation_response(rt.dashboard_storage_cache_cleanup(request).await)}
pub async fn storage_compact(State(rt):State<Arc<Runtime>>)->Response{mutation_response(rt.dashboard_storage_compact().await)}
pub async fn storage_health(State(rt):State<Arc<Runtime>>)->Response{match rt.dashboard_storage_health(){Ok(value)=>Json(value).into_response(),Err(error)=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":crate::error::payload(&error)}))).into_response()}}
pub async fn sandbox_diagnostics(State(rt):State<Arc<Runtime>>)->Response{match rt.dashboard_sandbox_diagnostics().await{Ok(value)=>Json(value).into_response(),Err(error)=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":crate::error::payload(&error)}))).into_response()}}
#[derive(Deserialize)]pub struct TasksQuery{pub limit:Option<usize>,pub auto_limit:Option<usize>}
pub async fn tasks(State(rt):State<Arc<Runtime>>,Query(query):Query<TasksQuery>)->Response{let explicit_limit=query.limit.unwrap_or(120).clamp(1,200);let auto_limit=query.auto_limit.unwrap_or(20).min(100);match crate::tools::tasks::dashboard_list(&rt.db,explicit_limit,auto_limit){Ok(mut value)=>{value["generated_at"]=json!(crate::util::now());Json(value).into_response()},Err(error)=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("{error:#}")}))).into_response()}}
#[derive(Deserialize)]pub struct OperationsQuery{pub limit:Option<usize>}
pub async fn operations(State(rt):State<Arc<Runtime>>,Query(query):Query<OperationsQuery>)->Response{match rt.db.operations(query.limit.unwrap_or(100)){Ok(items)=>Json(json!({"generated_at":crate::util::now(),"operations":items})).into_response(),Err(error)=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":format!("{error:#}")}))).into_response()}}
pub async fn operation(State(rt):State<Arc<Runtime>>,AxumPath(seq):AxumPath<i64>)->Response{match rt.db.operation(seq){Ok(value)=>Json(value).into_response(),Err(error)=>(StatusCode::NOT_FOUND,Json(json!({"error":format!("{error:#}")}))).into_response()}}
pub async fn job_detail(State(rt):State<Arc<Runtime>>,AxumPath(job_id):AxumPath<String>)->Response{let job=rt.jobs.get(&job_id);let output=rt.jobs.output(crate::tools::types::OutputArgs{job_id:job_id.clone(),offset:0,limit:32768});match(job,output){(Ok(job),Ok(output))=>Json(json!({"job":job,"output":output})).into_response(),(Err(error),_)|(_,Err(error))=>(StatusCode::NOT_FOUND,Json(json!({"error":format!("{error:#}")}))).into_response()}}
pub async fn events(State(rt):State<Arc<Runtime>>)->Sse<impl tokio_stream::Stream<Item=Result<Event,Infallible>>>{
    let stream=BroadcastStream::new(rt.subscribe_dashboard()).filter_map(|message|match message{Ok(data)=>Some(Ok::<Event,Infallible>(Event::default().data(data))),Err(_)=>None});
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keepalive"))
}
fn mutation_response(result:anyhow::Result<Value>)->Response{
    match result{
        Ok(value)=>Json(value).into_response(),
        Err(error)=>{
            let status=match crate::error::code(&error){"CONFIG_CONFLICT"|"FILE_CONFLICT"|"PATCH_CONFLICT"|"GIT_CONFLICT"|"STAGED_CONFLICT"|"PROJECT_BUSY"|"GIT_BUSY"|"CACHE_BUSY"|"IDEMPOTENCY_CONFLICT"|"RELOAD_RESTART_REQUIRED"=>StatusCode::CONFLICT,"PROJECT_READ_ONLY"|"PROJECT_EXEC_DISABLED"|"GIT_COMMIT_DISABLED"|"GIT_PUSH_DISABLED"=>StatusCode::FORBIDDEN,"WORKSPACE_NOT_AUTHORIZED"|"PROJECT_NOT_AUTHORIZED"=>StatusCode::NOT_FOUND,_=>StatusCode::BAD_REQUEST};
            (status,Json(crate::error::payload(&error))).into_response()
        }
    }
}
pub async fn add_project(State(rt):State<Arc<Runtime>>,Json(request):Json<AddProjectRequest>)->Response{mutation_response(rt.dashboard_add_project(request))}
pub async fn update_project(State(rt):State<Arc<Runtime>>,AxumPath((workspace,project)):AxumPath<(String,String)>,Json(request):Json<UpdateProjectRequest>)->Response{mutation_response(rt.dashboard_update_project(&workspace,&project,request))}
