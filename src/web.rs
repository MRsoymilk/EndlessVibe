use crate::runtime::Runtime;
use axum::{extract::State,http::{header,StatusCode},response::{Html,IntoResponse},Json};
use std::sync::Arc;
pub async fn home()->impl IntoResponse{Html(include_str!("../web/index.html"))}
pub async fn css()->impl IntoResponse{([(header::CONTENT_TYPE,"text/css; charset=utf-8")],include_str!("../web/app.css"))}
pub async fn javascript()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/app.js"))}
pub async fn uplot_javascript()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/vendor/uPlot/uPlot.iife.min.js"))}
pub async fn uplot_css()->impl IntoResponse{([(header::CONTENT_TYPE,"text/css; charset=utf-8")],include_str!("../web/vendor/uPlot/uPlot.min.css"))}
pub async fn favicon()->impl IntoResponse{StatusCode::NO_CONTENT}
pub async fn status(State(rt):State<Arc<Runtime>>)->impl IntoResponse{Json(rt.snapshot())}
pub async fn metrics(State(rt):State<Arc<Runtime>>)->impl IntoResponse{match rt.db.dashboard_metrics(3600,60){Ok(value)=>Json(value).into_response(),Err(error)=>(StatusCode::INTERNAL_SERVER_ERROR,format!("Dashboard metrics unavailable: {error}")).into_response()}}
pub async fn activity(State(rt):State<Arc<Runtime>>)->impl IntoResponse{let audits=rt.db.audits(50);let jobs=rt.jobs.list(crate::tools::types::ListJobsArgs{workspace:None,project:None,limit:30});match(audits,jobs){(Ok(audits),Ok(jobs))=>Json(serde_json::json!({"generated_at":crate::util::now(),"audits":audits,"jobs":jobs["jobs"]})).into_response(),(Err(error),_)|(_,Err(error))=>(StatusCode::INTERNAL_SERVER_ERROR,format!("Dashboard activity unavailable: {error}")).into_response()}}
pub async fn config(State(rt):State<Arc<Runtime>>)->impl IntoResponse{Json(rt.dashboard_config())}
