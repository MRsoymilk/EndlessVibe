use crate::runtime::Runtime;
use axum::{extract::State,http::{header,StatusCode},response::{Html,IntoResponse},Json};
use std::sync::Arc;
pub async fn home()->impl IntoResponse{Html(include_str!("../web/index.html"))}
pub async fn css()->impl IntoResponse{([(header::CONTENT_TYPE,"text/css; charset=utf-8")],include_str!("../web/app.css"))}
pub async fn javascript()->impl IntoResponse{([(header::CONTENT_TYPE,"text/javascript; charset=utf-8")],include_str!("../web/app.js"))}
pub async fn favicon()->impl IntoResponse{StatusCode::NO_CONTENT}
pub async fn status(State(rt):State<Arc<Runtime>>)->impl IntoResponse{Json(rt.snapshot())}
