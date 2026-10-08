use crate::{mcp::EndlessVibeMcp,runtime::Runtime,security::auth,web};
use axum::{body::{to_bytes,Body,HttpBody},extract::{DefaultBodyLimit,Request,State},http::{header,HeaderValue,Method,StatusCode},middleware::{self,Next},response::{IntoResponse,Response},routing::{get,patch,post,put},Router};
use rmcp::transport::streamable_http_server::{session::local::LocalSessionManager,StreamableHttpServerConfig,StreamableHttpService};
use std::{sync::Arc,time::Instant};

fn explicit_origin(value:&str)->String{
    let Ok(url)=url::Url::parse(value)else{return value.to_owned();};
    if !matches!(url.scheme(),"http"|"https"){return url.origin().ascii_serialization();}
    let Some(host)=url.host_str()else{return url.origin().ascii_serialization();};
    let Some(port)=url.port_or_known_default()else{return url.origin().ascii_serialization();};
    let host=host.trim_matches(['[',']']);let host=if host.contains(':'){format!("[{host}]")}else{host.to_owned()};
    format!("{}://{}:{port}",url.scheme(),host)
}

pub fn create_router(rt:Arc<Runtime>)->Router{
    let tool_state=rt.clone();let origin=explicit_origin(&rt.config.server.public_url);
    let transport=StreamableHttpServerConfig::default().with_legacy_session_mode(false).with_json_response(true).with_allowed_hosts(rt.config.hosts()).with_allowed_origins(vec![origin,"https://chatgpt.com:443".into(),"http://localhost:*".into(),"http://127.0.0.1:*".into()]).with_max_request_body_bytes(rt.body_limit()).with_cancellation_token(rt.shutdown.clone());
    let mcp=StreamableHttpService::new(move||Ok(EndlessVibeMcp::new(tool_state.clone())),LocalSessionManager::default().into(),transport);
    let protected:Router<Arc<Runtime>>=Router::new().route_service("/mcp",mcp.clone()).route_service("/mcp/",mcp).route_layer(middleware::from_fn_with_state(rt.clone(),auth::protect));
    let oauth=Router::new().route("/.well-known/oauth-authorization-server",get(auth::oauth_metadata)).route("/.well-known/oauth-protected-resource",get(auth::protected_metadata)).route("/.well-known/oauth-protected-resource/mcp",get(auth::protected_metadata)).route("/oauth/register",post(auth::register)).route("/oauth/authorize",get(auth::authorize).post(auth::consent)).route("/oauth/token",post(auth::token)).route("/oauth/revoke",post(auth::revoke)).layer(DefaultBodyLimit::max(16384));
    Router::new().route("/health",get(web::status)).merge(oauth).merge(protected).with_state(rt.clone()).layer(middleware::from_fn_with_state(rt,headers_and_logging))
}

pub fn create_dashboard_router(rt:Arc<Runtime>)->Router{
    Router::new()
        .route("/",get(web::home))
        .route("/projects",get(web::home))
        .route("/tasks",get(web::home))
        .route("/activity",get(web::home))
        .route("/operations",get(web::home))
        .route("/mcp",get(web::home))
        .route("/config",get(web::home))
        .route("/assets/app.css",get(web::css))
        .route("/assets/app.js",get(web::javascript))
        .route("/assets/js/common.js",get(web::javascript_common))
        .route("/assets/js/mcp.js",get(web::javascript_mcp))
        .route("/assets/js/charts.js",get(web::javascript_charts))
        .route("/vendor/uPlot/uPlot.min.css",get(web::uplot_css))
        .route("/vendor/uPlot/uPlot.iife.min.js",get(web::uplot_javascript))
        .route("/favicon.ico",get(web::favicon))
        .route("/api/status",get(web::status))
        .route("/api/metrics",get(web::metrics))
        .route("/api/activity",get(web::activity))
        .route("/api/config",get(web::config))
        .route("/api/config/reload",post(web::reload_config))
        .route("/api/config/execution/readonly-mounts",put(web::update_readonly_mounts))
        .route("/api/config/git",put(web::update_git))
        .route("/api/config/limits",put(web::update_limits))
        .route("/api/storage",get(web::storage_health))
        .route("/api/sandbox",get(web::sandbox_diagnostics))
        .route("/api/storage/maintenance",post(web::storage_maintenance))
        .route("/api/storage/cleanup",post(web::storage_cache_cleanup))
        .route("/api/storage/compact",post(web::storage_compact))
        .route("/api/projects/status",get(web::project_states))
        .route("/api/tasks",get(web::tasks))
        .route("/api/operations",get(web::operations))
        .route("/api/operations/{seq}",get(web::operation))
        .route("/api/jobs/{job_id}",get(web::job_detail))
        .route("/api/events",get(web::events))
        .route("/api/projects",post(web::add_project))
        .route("/api/projects/{workspace}/{project}",patch(web::update_project))
        .route("/health",get(web::status))
        .with_state(rt)
        .layer(DefaultBodyLimit::max(65536))
        .layer(middleware::from_fn(dashboard_headers))
}

async fn dashboard_headers(request:Request,next:Next)->Response{
    let authority=request.headers().get(header::HOST).and_then(|v|v.to_str().ok()).or_else(||request.uri().authority().map(|a|a.as_str()));
    let local_host=authority.and_then(|s|s.parse::<axum::http::uri::Authority>().ok()).is_some_and(|a|matches!(a.host().trim_matches(['[',']']),"127.0.0.1"|"localhost"|"::1"));
    if !local_host{return (StatusCode::FORBIDDEN,"Dashboard host is not allowed").into_response();}
    if request.method()!=Method::GET&&request.method()!=Method::HEAD{let origin=request.headers().get(header::ORIGIN).and_then(|v|v.to_str().ok());if !matches!(origin,Some("http://127.0.0.1:20001"|"http://localhost:20001"|"http://[::1]:20001")){return (StatusCode::FORBIDDEN,"Dashboard write origin is not allowed").into_response();}}
    let mut response=next.run(request).await;
    let headers=response.headers_mut();

    headers.insert(header::CACHE_CONTROL,HeaderValue::from_static("no-store"));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff"));
    headers.insert(header::REFERRER_POLICY,HeaderValue::from_static("no-referrer"));
    headers.insert(header::X_FRAME_OPTIONS,HeaderValue::from_static("DENY"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'"
        )
    );

    response
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
    let start=Instant::now();let method=request.method().clone();let path=request.uri().path().to_owned();let request_bytes=request.body().size_hint().exact().or_else(||request.headers().get(header::CONTENT_LENGTH).and_then(|v|v.to_str().ok()).and_then(|v|v.parse::<u64>().ok())).unwrap_or(0);
    let authority=request.headers().get(header::HOST).and_then(|v|v.to_str().ok()).or_else(||request.uri().authority().map(|a|a.as_str()));
    let allowed=authority.and_then(|s|s.parse::<axum::http::uri::Authority>().ok()).is_some_and(|a|rt.config.hosts().iter().any(|h|h.eq_ignore_ascii_case(a.host().trim_matches(['[',']']))));
    if !allowed{return (StatusCode::FORBIDDEN,"Host is not allowed").into_response();}
    if request.uri().to_string().len()>8192{return StatusCode::URI_TOO_LONG.into_response();}
    let mut response=next.run(request).await;let response_bytes=response.body().size_hint().exact().or_else(||response.headers().get(header::CONTENT_LENGTH).and_then(|v|v.to_str().ok()).and_then(|v|v.parse::<u64>().ok())).unwrap_or(0);rt.record_http_traffic(request_bytes,response_bytes);let h=response.headers_mut();
    h.insert(header::CACHE_CONTROL,HeaderValue::from_static("no-store"));h.insert(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff"));h.entry(header::REFERRER_POLICY).or_insert(HeaderValue::from_static("no-referrer"));h.insert(header::X_FRAME_OPTIONS,HeaderValue::from_static("DENY"));let mut form_origins=vec!["https://chatgpt.com".to_owned()];for redirect in &rt.config.security.extra_redirect_uris{if let Ok(u)=url::Url::parse(redirect){if matches!(u.scheme(),"http"|"https"){form_origins.push(u.origin().ascii_serialization());}}}form_origins.sort();form_origins.dedup();
    let csp=format!("default-src 'none'; style-src 'self'; script-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self' {}",form_origins.join(" "));
    if let Ok(value)=HeaderValue::from_str(&csp){h.insert(header::CONTENT_SECURITY_POLICY,value);}
    // Do not log queries, headers, tokens, form fields, command arguments or file contents.
    tracing::info!(http_method=%method,path=%path,status=response.status().as_u16(),elapsed_ms=start.elapsed().as_millis() as u64,rx_bytes=request_bytes,tx_bytes=response_bytes,"HTTP request");response
}

#[cfg(test)]
mod tests{
    use super::explicit_origin;
    #[test]
    fn allowed_origin_uses_explicit_ports(){
        assert_eq!(explicit_origin("https://endlessvibe.soymilk.xin"),"https://endlessvibe.soymilk.xin:443");
        assert_eq!(explicit_origin("http://localhost"),"http://localhost:80");
        assert_eq!(explicit_origin("https://example.test:8443/path"),"https://example.test:8443");
        assert_eq!(explicit_origin("http://[::1]"),"http://[::1]:80");
    }
}
