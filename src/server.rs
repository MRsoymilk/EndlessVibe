use crate::{mcp::EndlessVibeMcp,runtime::Runtime,security::auth,web};
use axum::{body::{to_bytes,Body},extract::{DefaultBodyLimit,Request,State},http::{header,HeaderValue,StatusCode},middleware::{self,Next},response::{IntoResponse,Response},routing::{get,post},Router};
use rmcp::transport::streamable_http_server::{session::local::LocalSessionManager,StreamableHttpServerConfig,StreamableHttpService};
use std::{sync::Arc,time::Instant};

pub fn create_router(rt:Arc<Runtime>)->Router{
    let tool_state=rt.clone();let origin=rt.config.public_url().map(|u|u.origin().ascii_serialization()).unwrap_or_else(|_|rt.config.server.public_url.clone());
    let transport=StreamableHttpServerConfig::default().with_legacy_session_mode(false).with_json_response(true).with_allowed_hosts(rt.config.hosts()).with_allowed_origins(vec![origin,"https://chatgpt.com:443".into(),"http://localhost:*".into(),"http://127.0.0.1:*".into()]).with_max_request_body_bytes(rt.body_limit()).with_cancellation_token(rt.shutdown.clone());
    let mcp=StreamableHttpService::new(move||Ok(EndlessVibeMcp::new(tool_state.clone())),LocalSessionManager::default().into(),transport);
    let protected:Router<Arc<Runtime>>=Router::new().route_service("/mcp",mcp.clone()).route_service("/mcp/",mcp).route_layer(middleware::from_fn_with_state(rt.clone(),auth::protect));
    let oauth=Router::new().route("/.well-known/oauth-authorization-server",get(auth::oauth_metadata)).route("/.well-known/oauth-protected-resource",get(auth::protected_metadata)).route("/.well-known/oauth-protected-resource/mcp",get(auth::protected_metadata)).route("/oauth/register",post(auth::register)).route("/oauth/authorize",get(auth::authorize).post(auth::consent)).route("/oauth/token",post(auth::token)).route("/oauth/revoke",post(auth::revoke)).layer(DefaultBodyLimit::max(16384));
    Router::new().route("/",get(web::home)).route("/assets/app.css",get(web::css)).route("/assets/app.js",get(web::javascript)).route("/favicon.ico",get(web::favicon)).route("/api/status",get(web::status)).route("/health",get(web::status)).merge(oauth).merge(protected).with_state(rt.clone()).layer(middleware::from_fn_with_state(rt,headers_and_logging))
}
pub(crate) async fn tool_descriptors(response:Response,limit:usize)->Response{
    if response.status()!=StatusCode::OK{return response;}
    let (mut parts,body)=response.into_parts();
    let bytes=match to_bytes(body,limit).await{Ok(bytes)=>bytes,Err(_)=>return StatusCode::INTERNAL_SERVER_ERROR.into_response()};
    let body=if let Ok(mut value)=serde_json::from_slice::<serde_json::Value>(&bytes){crate::mcp::publish_security_schemes(&mut value);Body::from(value.to_string())}else{Body::from(bytes)};
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts,body)
}
async fn headers_and_logging(State(rt):State<Arc<Runtime>>,request:Request,next:Next)->Response{
    let start=Instant::now();let method=request.method().clone();let path=request.uri().path().to_owned();
    let authority=request.headers().get(header::HOST).and_then(|v|v.to_str().ok()).or_else(||request.uri().authority().map(|a|a.as_str()));
    let allowed=authority.and_then(|s|s.parse::<axum::http::uri::Authority>().ok()).is_some_and(|a|rt.config.hosts().iter().any(|h|h.eq_ignore_ascii_case(a.host().trim_matches(['[',']']))));
    if !allowed{return (StatusCode::FORBIDDEN,"Host is not allowed").into_response();}
    if request.uri().to_string().len()>8192{return StatusCode::URI_TOO_LONG.into_response();}
    let mut response=next.run(request).await;let h=response.headers_mut();
    h.insert(header::CACHE_CONTROL,HeaderValue::from_static("no-store"));h.insert(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff"));h.entry(header::REFERRER_POLICY).or_insert(HeaderValue::from_static("no-referrer"));h.insert(header::X_FRAME_OPTIONS,HeaderValue::from_static("DENY"));let mut form_origins=vec!["https://chatgpt.com".to_owned()];for redirect in &rt.config.security.extra_redirect_uris{if let Ok(u)=url::Url::parse(redirect){if matches!(u.scheme(),"http"|"https"){form_origins.push(u.origin().ascii_serialization());}}}form_origins.sort();form_origins.dedup();
    let csp=format!("default-src 'none'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self' {}",form_origins.join(" "));
    if let Ok(value)=HeaderValue::from_str(&csp){h.insert(header::CONTENT_SECURITY_POLICY,value);}
    // Do not log queries, headers, tokens, form fields, command arguments or file contents.
    tracing::info!(http_method=%method,path=%path,status=response.status().as_u16(),elapsed_ms=start.elapsed().as_millis() as u64,"HTTP request");response
}
