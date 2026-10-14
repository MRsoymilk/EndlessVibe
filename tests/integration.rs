//! Integration tests run by cargo test. All projects and credentials are temporary.
use axum::{body::{to_bytes,Body},http::{header,Request,StatusCode},Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
use endlessvibe::{config::{Config,ProjectConfig,WorkspaceConfig},mcp,runtime::Runtime,server,tools::{filesystem,git,tasks,types::*},util};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::{path::Path,sync::Arc,time::Duration};
use tower::ServiceExt;


#[tokio::test]
async fn brand_icon_is_a_real_png_on_dashboard_and_public_oauth_routes(){
 let f=fixture(|_|{});
 let reference=include_bytes!("../EndlessVibe.png");
 assert!(reference.starts_with(b"\x89PNG\r\n\x1a\n"));
 assert!(reference.len()>1000);
 assert!(matches!(reference[25],4|6),"Brand icon PNG must have transparency");
 for (site,router) in [
  ("dashboard",server::create_dashboard_router(f.rt.clone())),
  ("oauth",server::create_router(f.rt.clone()))
 ]{
  for path in ["/assets/EndlessVibe.png","/favicon.ico"]{
   let request=Request::builder().uri(path).header(header::HOST,"localhost").body(Body::empty()).unwrap();
   let response=router.clone().oneshot(request).await.unwrap();
   assert_eq!(response.status(),StatusCode::OK,"{site} {path}");
   assert_eq!(response.headers().get(header::CONTENT_TYPE).unwrap(),"image/png");
   let actual=to_bytes(response.into_body(),2*1024*1024).await.unwrap();
   assert_eq!(actual.as_ref(),reference,"{site} {path}");
  }
 }
 let width=u32::from_be_bytes(reference[16..20].try_into().unwrap());
 let height=u32::from_be_bytes(reference[20..24].try_into().unwrap());
 let dashboard=server::create_dashboard_router(f.rt.clone());
 let page=dashboard.clone().oneshot(Request::builder().uri("/").header(header::HOST,"localhost").body(Body::empty()).unwrap()).await.unwrap();
 assert_eq!(page.status(),StatusCode::OK);
 let html=String::from_utf8(to_bytes(page.into_body(),2*1024*1024).await.unwrap().to_vec()).unwrap();
 assert!(html.contains("class=\"brand-icon\""));
 assert!(html.contains("rel=\"icon\""));
 assert!(html.contains("rel=\"apple-touch-icon\""));
 assert!(html.contains("href=\"/manifest.webmanifest\""));
 let manifest_response=dashboard.oneshot(Request::builder().uri("/manifest.webmanifest").header(header::HOST,"localhost").body(Body::empty()).unwrap()).await.unwrap();
 assert_eq!(manifest_response.status(),StatusCode::OK);
 assert!(manifest_response.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap().starts_with("application/manifest+json"));
 let manifest:Value=serde_json::from_slice(&to_bytes(manifest_response.into_body(),65536).await.unwrap()).unwrap();
 assert_eq!(manifest["name"],"EndlessVibe");
 assert_eq!(manifest["icons"][0]["src"],"/assets/EndlessVibe.png");
 assert_eq!(manifest["icons"][0]["sizes"].as_str().unwrap(),format!("{width}x{height}"));
 assert!(include_str!("../src/security/auth.rs").contains("class=\"brand-icon oauth-icon\""));
}
#[test]
fn dashboard_starts_with_generic_paths_and_config_defaults(){
    let html=include_str!("../web/index.html");
    assert!(html.contains("id=\"add-project-path\" type=\"text\" required placeholder=\"请输入现有项目的绝对路径\""));
    assert!(!html.contains("placeholder=\"/home/"));
    assert!(!html.contains("placeholder=\"D:"));
    let app=include_str!("../web/app.js");
    assert!(!app.contains("updateProjectPathPlaceholder"));
    // An earlier front-end patch accidentally placed a // comment on this
    // compact one-line handler, commenting out its remaining validation.
    assert!(app.contains("if(!path){setWriteStatus(\"Absolute Path 不能为空\",\"error\");return;}if(project"));
    assert_eq!(Config::default().server.public_url,"https://mcp.example.com");
}


#[tokio::test]
async fn setup_router_and_normal_dashboard_have_separate_surfaces(){
    let f=fixture(|_|{});
    let setup=server::create_setup_router(f.rt.clone());
    for url in ["/","/setup"]{
        let response=http(&setup,"GET",url,Body::empty(),None,None,None).await;
        assert_eq!(response.status(),StatusCode::OK);
        let html=String::from_utf8(to_bytes(response.into_body(),128*1024).await.unwrap().to_vec()).unwrap();
        for marker in ["id=\"wizard-progress\"","data-wizard-step=\"0\"",
            "data-wizard-step=\"1\"","data-wizard-step=\"2\"","data-wizard-step=\"3\"",
            "aria-labelledby=\"setup-workspaces-title\" hidden"]{
            assert!(html.contains(marker),"{url} missing {marker}");
        }
        assert!(!html.contains("class=\"top-nav\"")&&!html.contains("href=\"/projects\""));
    }
    for path in ["/projects","/config","/nodes","/api/config","/api/tasks",
        "/api/nodes","/mcp","/oauth/token","/assets/app.js"]{
        assert_eq!(http(&setup,"GET",path,Body::empty(),None,None,None).await.status(),
            StatusCode::NOT_FOUND,"Setup exposes {path}");
    }
    for path in ["/api/setup","/assets/js/setup.js","/assets/setup.css"]{
        assert_eq!(http(&setup,"GET",path,Body::empty(),None,None,None).await.status(),
            StatusCode::OK,"Wizard lacks {path}");
    }
    let normal=server::create_dashboard_router(f.rt.clone());
    for path in ["/setup","/api/setup","/assets/js/setup.js","/assets/setup.css"]{
        assert_eq!(http(&normal,"GET",path,Body::empty(),None,None,None).await.status(),
            StatusCode::NOT_FOUND,"Normal Dashboard exposes {path}");
    }
    assert_eq!(http(&normal,"GET","/api/workspaces",Body::empty(),None,None,None).await.status(),
        StatusCode::METHOD_NOT_ALLOWED,"Workspace changes must use POST");
    let page=http(&normal,"GET","/",Body::empty(),None,None,None).await;
    let html=String::from_utf8(to_bytes(page.into_body(),128*1024).await.unwrap().to_vec()).unwrap();
    assert!(!html.contains("data-page=\"setup\"")&&!html.contains("href=\"/setup\""));
}
#[tokio::test]
async fn normal_dashboard_creates_workspace_with_hot_reload_and_local_security(){
    let f=fixture(|_|{});
    let dashboard=server::create_dashboard_router(f.rt.clone());
    let external=server::create_router(f.rt.clone());
    let root=f._dir.path().join("research");
    let project_path=root.join("analysis");
    std::fs::create_dir_all(&project_path).unwrap();
    let page=String::from_utf8(to_bytes(
        http(&dashboard,"GET","/projects",Body::empty(),None,None,None)
            .await.into_body(),128*1024).await.unwrap().to_vec()).unwrap();
    for id in ["add-workspace-form","add-workspace-id","add-workspace-path",
        "add-workspace-save","workspace-write-status"]{
        assert!(page.contains(&format!("id=\"{id}\"")),"Projects page missing {id}");
    }
    let script=include_str!("../web/app.js");
    assert!(script.contains("handleAddWorkspace(event)"));
    assert!(script.contains("sendJson(\"/api/workspaces\",\"POST\""));
    assert!(script.contains("expected_revision:configRevision,id,path"));
    assert!(script.contains("select.value=id"));
    let initial=json_body(http(&dashboard,"GET","/api/config",
        Body::empty(),None,None,None).await).await;
    assert_eq!(initial["workspaces"].as_array().unwrap().len(),1);
    let body=json!({"expected_revision":initial["revision"],"id":"research","path":root});
    let post=|host:&str,origin:Option<&str>,data:Value|{
        let mut request=Request::builder().method("POST").uri("/api/workspaces")
            .header(header::HOST,host).header(header::CONTENT_TYPE,"application/json");
        if let Some(origin)=origin {request=request.header(header::ORIGIN,origin);}
        request.body(Body::from(data.to_string())).unwrap()
    };
    let before=std::fs::read(&f.rt.config_path).unwrap();
    for (host,origin) in [("localhost",None),
        ("localhost",Some("https://outside.example")),
        ("outside.example",Some("http://localhost:20001"))]{
        let result=dashboard.clone().oneshot(post(host,origin,body.clone())).await.unwrap();
        assert_eq!(result.status(),StatusCode::FORBIDDEN);
    }
    assert_eq!(std::fs::read(&f.rt.config_path).unwrap(),before);
    assert_eq!(external.clone().oneshot(post("localhost",Some("http://localhost:20001"),
        body.clone())).await.unwrap().status(),StatusCode::NOT_FOUND);
    let saved=dashboard.clone().oneshot(post("localhost",
        Some("http://localhost:20001"),body.clone())).await.unwrap();
    assert_eq!(saved.status(),StatusCode::OK);
    let saved=json_body(saved).await;
    assert_eq!(saved["requires_restart"],false);
    assert_eq!(saved["workspace"]["id"],"research");
    assert_eq!(saved["workspace"]["project_count"],0);
    assert_eq!(f.rt.workspace_project_counts(),(2,1));
    assert!(f.rt.project("research","analysis").is_err(),
        "Workspace creation must not authorize any Project");
    let current=json_body(http(&dashboard,"GET","/api/config",
        Body::empty(),None,None,None).await).await;
    assert_eq!(current["requires_restart"],false);
    assert_eq!(current["workspaces"].as_array().unwrap().len(),2);
    assert_eq!(current["active_revision"],current["revision"]);
    assert!(current["workspaces"].as_array().unwrap().iter().any(|w|
        w["id"]=="research" && w["project_count"]==0));
    let status=json_body(http(&dashboard,"GET","/api/status",
        Body::empty(),None,None,None).await).await;
    assert_eq!(status["workspace_count"],2);
    assert_eq!(status["project_count"],1);
    assert_eq!(dashboard.clone().oneshot(post("localhost",
        Some("http://localhost:20001"),body)).await.unwrap().status(),
        StatusCode::CONFLICT,"Stale revision must not overwrite configuration");
    let revision=current["revision"].clone();
    for (id,path) in [("research",root.clone()),("nested",root.join("analysis")),
        ("state",f.rt.config.security.data_dir.clone())] {
        let response=dashboard.clone().oneshot(post("localhost",
            Some("http://localhost:20001"),
            json!({"expected_revision":revision,"id":id,"path":path}))).await.unwrap();
        assert_eq!(response.status(),StatusCode::BAD_REQUEST,"{id} should be rejected");
    }
    assert_eq!(f.rt.workspace_project_counts(),(2,1));
    // New Workspace is immediately selectable for ordinary Project management.
    let request=Request::builder().method("POST").uri("/api/projects")
        .header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001")
        .header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(json!({"expected_revision":revision,"workspace":"research",
            "project":"analysis","path":project_path,"allow_write":false,
            "allow_exec":false,"allow_git_commit":false,"execution_profile":"isolated"})
            .to_string())).unwrap();
    let added=dashboard.clone().oneshot(request).await.unwrap();
    assert_eq!(added.status(),StatusCode::OK);
    let added=json_body(added).await;
    assert_eq!(added["requires_restart"],false);
    assert_eq!(f.rt.workspace_project_counts(),(2,2));
    assert!(f.rt.project("research","analysis").is_ok());
}

#[tokio::test]
async fn dashboard_setup_is_local_and_distinguishes_saved_from_active_connection(){
    let f=fixture(|c|{let state=c.security.data_dir.clone();*c=Config::local_setup();c.security.data_dir=state;});
    let app=server::create_setup_router(f.rt.clone());let public=server::create_router(f.rt.clone());
    let initial=json_body(http(&app,"GET","/api/setup",Body::empty(),None,None,None).await).await;
    assert_eq!(initial["needs_setup"],true);assert_eq!(initial["requires_restart"],false);
    assert_eq!(initial["workspace_count"],0);assert_eq!(initial["active"]["bind"],"127.0.0.1:20000");
    assert_eq!(initial["public_reachability"],"not_checked");assert_eq!(initial["chatgpt_connection"],"not_verified");
    assert!(!initial.to_string().contains(&f.owner));assert!(!f.rt.config.transfer.enabled);
    for path in ["/setup","/api/setup","/assets/js/setup.js"]{
        let response=http(&app,"GET",path,Body::empty(),None,None,None).await;
        assert_eq!(response.status(),StatusCode::OK,"{path}");
        assert_eq!(response.headers()[header::CACHE_CONTROL],"no-store");
        assert_eq!(http(&public,"GET",path,Body::empty(),None,None,None).await.status(),StatusCode::NOT_FOUND,"{path} must remain local");
    }
    let page=http(&app,"GET","/setup",Body::empty(),None,None,None).await;
    let html=String::from_utf8(to_bytes(page.into_body(),2*1024*1024).await.unwrap().to_vec()).unwrap();
    assert!(html.contains("data-wizard-step=\"0\""));
    let put=|origin:Option<&str>,body:Value|{
        let mut request=Request::builder().method("PUT").uri("/api/setup").header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json");
        if let Some(origin)=origin{request=request.header(header::ORIGIN,origin);}
        request.body(Body::from(body.to_string())).unwrap()
    };
    let body=json!({"expected_revision":initial["revision"],"public_url":"Demo.ngrok-free.app","connection_mode":"loopback"});
    for origin in [None,Some("https://outside.ngrok.app")]{assert_eq!(app.clone().oneshot(put(origin,body.clone())).await.unwrap().status(),StatusCode::FORBIDDEN);}
    let owner_before=std::fs::read(f.rt.config.owner_key_path()).unwrap();
    let saved=app.clone().oneshot(put(Some("http://localhost:20001"),body.clone())).await.unwrap();assert_eq!(saved.status(),StatusCode::OK);
    let saved=json_body(saved).await;assert_eq!(saved["saved"]["mcp_endpoint"],"https://demo.ngrok-free.app/mcp");
    assert_eq!(saved["active"]["mcp_endpoint"],"http://127.0.0.1:20000/mcp");assert_eq!(saved["requires_restart"],true);
    assert_eq!(saved["needs_setup"],false);assert_eq!(saved["startup_override_detected"],false);
    assert_eq!(app.clone().oneshot(put(Some("http://localhost:20001"),body)).await.unwrap().status(),StatusCode::CONFLICT);
    assert_eq!(std::fs::read(f.rt.config.owner_key_path()).unwrap(),owner_before);assert_eq!(f.rt.workspace_project_counts(),(0,0));
    let metadata=json_body(http(&public,"GET","/.well-known/oauth-protected-resource",Body::empty(),None,None,None).await).await;
    assert_eq!(metadata["resource"],"http://127.0.0.1:20000/mcp","save must not change OAuth in the running process");
    let hidden=Request::builder().method("PUT").uri("/api/setup").header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001").header(header::CONTENT_TYPE,"application/json").body(Body::from("{}")).unwrap();
    assert_eq!(public.clone().oneshot(hidden).await.unwrap().status(),StatusCode::NOT_FOUND);
    let old_token=f.rt.auth.issue_local_token().unwrap();let config_path=f.rt.config_path.clone();drop(app);drop(public);
    let Fixture{_dir,rt,owner}=f;drop(rt);
    let restarted=Runtime::new(Config::load_file(&config_path).unwrap(),&config_path).unwrap();
    let applied=restarted.dashboard_setup().unwrap();assert_eq!(applied["requires_restart"],false);assert_eq!(applied["connection_matches_saved"],true);
    assert_eq!(applied["active"]["mcp_endpoint"],"https://demo.ngrok-free.app/mcp");
    assert_eq!(std::fs::read(restarted.config.owner_key_path()).unwrap(),owner_before);
    let router=server::create_router(restarted.clone());
    let request=Request::builder().uri("/.well-known/oauth-authorization-server").header(header::HOST,"demo.ngrok-free.app").body(Body::empty()).unwrap();
    let metadata=json_body(router.clone().oneshot(request).await.unwrap()).await;assert_eq!(metadata["issuer"],"https://demo.ngrok-free.app");
    let response=http(&router,"POST","/mcp",Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_workspaces","arguments":{}}}"#),Some("application/json"),Some(&old_token),None).await;
    assert_eq!(response.status(),StatusCode::OK);
    let challenge=response.headers()[header::WWW_AUTHENTICATE].to_str().unwrap().to_owned();
    assert!(challenge.contains("resource_metadata=\"https://demo.ngrok-free.app/.well-known/oauth-protected-resource\""));
    assert!(challenge.contains("error=\"invalid_token\""));
    let rejected=json_body(response).await;
    assert_eq!(rejected["result"]["isError"],true,"old origin tokens must not authorize the new domain");
    assert_eq!(rejected["result"]["_meta"]["mcp/www_authenticate"][0],challenge);
    assert!(rejected["result"]["structuredContent"].is_null());
    let f=Fixture{_dir,rt:restarted,owner};let (_,token)=oauth_login(&f,&router,"files:read").await;
    assert!(token["access_token"].is_string(),"new OAuth authorization must work after switching domains");
}
#[tokio::test]
async fn setup_wizard_sees_saved_projects_with_pending_connection_restart(){
    let f=fixture(|c|{let state=c.security.data_dir.clone();*c=Config::local_setup();c.security.data_dir=state;});
    let app=server::create_setup_router(f.rt.clone());
    let post=|verb:&str,url:&str,body:Value|Request::builder().method(verb).uri(url)
        .header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001")
        .header(header::CONTENT_TYPE,"application/json")
        .body(Body::from(body.to_string())).unwrap();
    let initial=f.rt.dashboard_setup().unwrap();
    let saved=app.clone().oneshot(post("PUT","/api/setup",json!({
        "expected_revision":initial["revision"],"public_url":"wizard.ngrok.app","connection_mode":"loopback"
    }))).await.unwrap();
    assert_eq!(saved.status(),StatusCode::OK);
    let saved=json_body(saved).await;
    assert_eq!(saved["requires_restart"],true);
    let root=f._dir.path().join("workspace");
    let added=app.clone().oneshot(post("POST","/api/workspaces",json!({
        "expected_revision":saved["revision"],"id":"newroot","path":root
    }))).await.unwrap();
    assert_eq!(added.status(),StatusCode::OK);
    let current=json_body(http(&app,"GET","/api/setup",Body::empty(),None,None,None).await).await;
    assert_eq!(current["workspace_count"],1);
    assert_eq!(current["workspaces"][0]["id"],"newroot");
    let project=app.clone().oneshot(post("POST","/api/projects",json!({
        "expected_revision":current["revision"],"workspace":"newroot","project":"app",
        "path":root.join("project"),"allow_write":false,"allow_exec":false,
        "allow_git_commit":false,"allow_git_mutation":false,"allow_git_push":false,
        "execution_profile":"isolated"
    }))).await.unwrap();
    assert_eq!(project.status(),StatusCode::OK);
    let status=json_body(http(&app,"GET","/api/setup",Body::empty(),None,None,None).await).await;
    assert_eq!(status["workspace_count"],1);
    assert_eq!(status["project_count"],1);
    assert_eq!(status["workspaces"][0]["projects"][0]["id"],"app");
    assert_eq!(status["active"]["public_url"],"http://127.0.0.1:20000");
    assert_eq!(status["saved"]["public_url"],"https://wizard.ngrok.app");
}
#[tokio::test]
async fn dashboard_setup_adds_first_workspace_without_implicit_project_access(){
    let f=fixture(|c|{let state=c.security.data_dir.clone();*c=Config::local_setup();c.security.data_dir=state;});
    let root=f._dir.path().join("workspace");let app=server::create_setup_router(f.rt.clone());
    let post=|path:&str,body:Value|Request::builder().method("POST").uri(path).header(header::HOST,"localhost")
        .header(header::ORIGIN,"http://localhost:20001").header(header::CONTENT_TYPE,"application/json").body(Body::from(body.to_string())).unwrap();
    let initial=f.rt.dashboard_setup().unwrap();
    let body=json!({"expected_revision":initial["revision"],"id":"work","path":root});
    let saved=app.clone().oneshot(post("/api/workspaces",body.clone())).await.unwrap();assert_eq!(saved.status(),StatusCode::OK);
    let saved=json_body(saved).await;assert_eq!(saved["requires_restart"],false);assert_eq!(saved["workspace"]["project_count"],0);
    assert_eq!(f.rt.workspace_project_counts(),(1,0));
    assert_eq!(app.clone().oneshot(post("/api/workspaces",body)).await.unwrap().status(),StatusCode::CONFLICT);
    let revision=saved["revision"].clone();
    for (id,path) in [("state",f.rt.config.security.data_dir.clone()),("config",f._dir.path().to_owned()),("overlap",root.join("project"))]{
        let response=app.clone().oneshot(post("/api/workspaces",json!({"expected_revision":revision,"id":id,"path":path}))).await.unwrap();
        assert_eq!(response.status(),StatusCode::BAD_REQUEST,"{id}");
    }
    let add=json!({"expected_revision":revision,"workspace":"work","project":"app","path":root.join("project"),"allow_write":true,"allow_exec":false,"allow_git_commit":false});
    let added=app.clone().oneshot(post("/api/projects",add)).await.unwrap();assert_eq!(added.status(),StatusCode::OK);
    assert_eq!(json_body(added).await["requires_restart"],false);assert_eq!(f.rt.workspace_project_counts(),(1,1));
    let project=f.rt.project("work","app").unwrap();assert!(project.config.allow_write);assert!(!project.config.allow_exec&&!project.config.allow_git_commit);
    let public=server::create_router(f.rt.clone());assert_eq!(public.oneshot(post("/api/workspaces",json!({}))).await.unwrap().status(),StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn dashboard_setup_identifies_startup_overrides_even_when_file_revision_is_current(){
    let f=fixture(|c|{let state=c.security.data_dir.clone();*c=Config::local_setup();c.security.data_dir=state;c.server.public_url="https://saved.ngrok.app".into();c.security.allow_http_loopback=false;});
    let path=f.rt.config_path.clone();let mut effective=(*f.rt.config).clone();effective.server.public_url="https://override.ngrok.app".into();
    let Fixture{_dir,rt,owner:_}=f;drop(rt);let rt=Runtime::new(effective,&path).unwrap();
    let status=rt.dashboard_setup().unwrap();assert_eq!(status["startup_override_detected"],true);
    assert_eq!(status["requires_restart"],true);assert_eq!(status["connection_matches_saved"],false);
    assert_eq!(status["saved"]["public_url"],"https://saved.ngrok.app");assert_eq!(status["active"]["public_url"],"https://override.ngrok.app");
    assert_eq!(status["revision"],endlessvibe::config_edit::revision(&path).unwrap());drop(rt);drop(_dir);
}
#[tokio::test]
async fn dashboard_execution_backend_requires_local_explicit_confirmation_and_restart(){
    let f=fixture(|c|{
        c.execution.backend="disabled".into();
        c.execution.acknowledge_unsafe_host_execution=false;
        c.execution.allow_shell=false;
    });
    let app=server::create_dashboard_router(f.rt.clone());
    let initial=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;
    assert_eq!(initial["execution"]["backend"],"disabled");
    assert_eq!(initial["execution"]["active_backend"],"disabled");
    assert_eq!(initial["execution"]["host_os"],std::env::consts::OS);
    let revision=initial["revision"].as_str().unwrap().to_owned();
    let request=|backend:&str,ack:bool,revision:&str,origin:&str|{
        Request::builder().method("PUT").uri("/api/config/execution/backend")
            .header(header::HOST,"localhost")
            .header(header::ORIGIN,origin)
            .header(header::CONTENT_TYPE,"application/json")
            .body(Body::from(json!({"expected_revision":revision,"backend":backend,
                "acknowledge_unsafe_host_execution":ack}).to_string())).unwrap()
    };
    let rejected=app.clone().oneshot(request("host",false,&revision,"http://localhost:20001")).await.unwrap();
    assert_eq!(rejected.status(),StatusCode::BAD_REQUEST);
    assert!(json_body(rejected).await["message"].as_str().unwrap().contains("acknowledgement"));
    assert_eq!(endlessvibe::config_edit::revision(&f.rt.config_path).unwrap(),revision);
    let origin_denied=app.clone().oneshot(request("host",true,&revision,"https://external.example")).await.unwrap();
    assert_eq!(origin_denied.status(),StatusCode::FORBIDDEN);
    assert_eq!(endlessvibe::config_edit::revision(&f.rt.config_path).unwrap(),revision);
    let confirmed=app.clone().oneshot(request("host",true,&revision,"http://localhost:20001")).await.unwrap();
    assert_eq!(confirmed.status(),StatusCode::OK);
    let saved=json_body(confirmed).await;
    assert_eq!(saved["requires_restart"],true);
    assert_eq!(saved["execution"]["backend"],"host");
    assert_eq!(saved["execution"]["acknowledge_unsafe_host_execution"],true);
    assert_eq!(f.rt.config.execution.backend,"disabled","save must not enable execution in memory");
    let after=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;
    assert_eq!(after["execution"]["backend"],"host");
    assert_eq!(after["execution"]["active_backend"],"disabled");
    assert_eq!(after["requires_restart"],true);
    assert_eq!(app.clone().oneshot(request("host",true,&revision,"http://localhost:20001")).await.unwrap().status(),StatusCode::CONFLICT);
    let current=saved["revision"].as_str().unwrap();
    assert_eq!(app.clone().oneshot(request("disabled",true,current,"http://localhost:20001")).await.unwrap().status(),StatusCode::BAD_REQUEST);
    let reverted=app.clone().oneshot(request("disabled",false,current,"http://localhost:20001")).await.unwrap();
    assert_eq!(reverted.status(),StatusCode::OK);
    let config=Config::load_file(&f.rt.config_path).unwrap();
    assert_eq!(config.execution.backend,"disabled");
    assert!(!config.execution.acknowledge_unsafe_host_execution);
    assert!(!config.execution.allow_shell);
    let public=server::create_router(f.rt.clone());
    let hidden=Request::builder().method("PUT").uri("/api/config/execution/backend")
        .header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001")
        .header(header::CONTENT_TYPE,"application/json").body(Body::from("{}")).unwrap();
    assert_eq!(public.oneshot(hidden).await.unwrap().status(),StatusCode::NOT_FOUND);
    let html=include_str!("../web/index.html");
    let js=include_str!("../web/app.js");
    for marker in ["id=\"execution-backend-editor\"","name=\"acknowledge_unsafe_host_execution\"",
                   "id=\"execution-host-risk\""]{assert!(html.contains(marker));}
    for marker in ["/api/config/execution/backend","window.confirm(","execution.host_os",
                   "executionBackendDirty","executionBackendSaving"]{assert!(js.contains(marker));}
}
#[tokio::test]
async fn child_one_click_approval_finalizes_parent_without_project_grants(){
    let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=socket.local_addr().unwrap();drop(socket);
    let child=fixture(|c|{
        c.transfer.enabled=true;
        c.transfer.listen=address;
        c.workspaces[0].projects.clear();
    });
    let parent=fixture(|c|c.transfer.enabled=true);
    let stop=tokio_util::sync::CancellationToken::new();
    let tls=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;

    let offer=parent.rt.transfer.start_pair(address).await.unwrap();
    let id=offer["id"].as_str().unwrap().to_owned();
    assert_eq!(parent.rt.transfer.local_role().unwrap(),"parent");
    assert_eq!(child.rt.transfer.local_role().unwrap(),"child");
    assert_eq!(parent.rt.transfer.discoveries()["local_role"],"parent");
    assert_eq!(child.rt.transfer.discoveries()["local_role"],"child");
    assert_eq!(child.rt.transfer.pending().unwrap()["pending"][0]["code"],offer["code"]);
    assert_eq!(child.rt.transfer.pending().unwrap()["pending"][0]["requires_local_confirmation"],true);
    assert!(parent.rt.transfer.peers().unwrap()["peers"].as_array().unwrap().is_empty());
    assert!(child.rt.transfer.peers().unwrap()["peers"].as_array().unwrap().is_empty());
    assert!(child.rt.transfer.start_pair(address).await.is_err());

    // Even with the correct offer ID, the parent is not permitted to approve itself.
    let bad=Request::builder().method("POST").uri("/api/nodes/approve")
        .header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json")
        .header(header::ORIGIN,"http://localhost:20001")
        .body(Body::from(json!({"id":id}).to_string())).unwrap();
    let parent_app=server::create_dashboard_router(parent.rt.clone());
    assert_eq!(parent_app.oneshot(bad).await.unwrap().status(),StatusCode::BAD_REQUEST);
    assert_eq!(parent.rt.transfer.reconcile_pending().await.unwrap()["pending"][0]["state"],"pending");

    // The Dashboard sends only the pairing ID, with no Project grant selection.
    let child_app=server::create_dashboard_router(child.rt.clone());
    let accept=Request::builder().method("POST").uri("/api/nodes/approve")
        .header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json")
        .header(header::ORIGIN,"http://localhost:20001")
        .body(Body::from(json!({"id":id}).to_string())).unwrap();
    assert_eq!(child_app.oneshot(accept).await.unwrap().status(),StatusCode::OK);

    let forged=endlessvibe::transfer::secure::Wire{
        kind:"pair_status".into(),node_id:parent.rt.transfer.node_id.clone(),
        name:String::new(),id:id.clone(),token:"wrong-secret".repeat(4),
        tool:String::new(),args:Value::Null
    };
    assert!(child.rt.transfer.pairing_status(&forged).is_err());
    assert_eq!(parent.rt.transfer.reconcile_pending().await.unwrap()["pending"].as_array().unwrap().len(),0);
    let trusted=parent.rt.transfer.peers().unwrap();
    assert_eq!(trusted["peers"][0]["role"],"parent");
    assert_eq!(trusted["peers"][0]["node_id"],child.rt.transfer.node_id);
    assert_eq!(child.rt.transfer.peers().unwrap()["peers"][0]["role"],"child");
    assert!(child.rt.transfer.peers().unwrap()["peers"][0]["grants"].as_array().unwrap().is_empty());
    let visible=parent.rt.transfer.call_node(&child.rt.transfer.node_id,"list_workspaces",json!({})).await.unwrap();
    assert!(visible["workspaces"].as_array().unwrap().is_empty());
    stop.cancel();tls.await.unwrap().unwrap();
}

#[tokio::test]
async fn child_rejection_propagates_to_parent_without_creating_trust(){
    let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=socket.local_addr().unwrap();drop(socket);
    let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});
    let parent=fixture(|c|c.transfer.enabled=true);
    let stop=tokio_util::sync::CancellationToken::new();
    let tls=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let offer=parent.rt.transfer.start_pair(address).await.unwrap();
    let id=offer["id"].as_str().unwrap().to_owned();
    let app=server::create_dashboard_router(child.rt.clone());
    let reject=Request::builder().method("POST").uri("/api/nodes/reject")
        .header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json")
        .header(header::ORIGIN,"http://localhost:20001")
        .body(Body::from(json!({"id":id}).to_string())).unwrap();
    let response=app.clone().oneshot(reject).await.unwrap();
    assert_eq!(response.status(),StatusCode::OK);
    assert_eq!(json_body(response).await["rejected"],true);
    assert_eq!(parent.rt.transfer.reconcile_pending().await.unwrap()["pending"][0]["state"],"rejected");
    assert!(child.rt.transfer.peers().unwrap()["peers"].as_array().unwrap().is_empty());
    assert!(parent.rt.transfer.peers().unwrap()["peers"].as_array().unwrap().is_empty());

    // A declined request cannot later be approved or turned into a live peer.
    let accept=Request::builder().method("POST").uri("/api/nodes/approve")
        .header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json")
        .header(header::ORIGIN,"http://localhost:20001")
        .body(Body::from(json!({"id":id}).to_string())).unwrap();
    assert_eq!(app.oneshot(accept).await.unwrap().status(),StatusCode::BAD_REQUEST);
    assert!(parent.rt.transfer.call_node(&child.rt.transfer.node_id,"list_workspaces",json!({})).await.is_err());
    stop.cancel();tls.await.unwrap().unwrap();
}

#[tokio::test]
async fn child_can_pair_before_registering_any_project(){
    let f=fixture(|c|{c.transfer.enabled=true;c.workspaces[0].projects.clear();});
    assert!(f.rt.projects_snapshot().is_empty());
    let parent="abcdef123456abcdef123456";
    let token="a".repeat(44);
    let offer=endlessvibe::transfer::secure::Wire{
        kind:"pair_hello".into(),node_id:parent.into(),name:"Linux Parent".into(),
        id:"123456789012345678901234".into(),token:token.clone(),
        tool:String::new(),args:Value::Null
    };
    f.rt.transfer.receive_pair(&offer,"127.0.0.1:23456".parse().unwrap(),"test-certificate").unwrap();
    let app=server::create_dashboard_router(f.rt.clone());
    let req=Request::builder().method("POST").uri("/api/nodes/approve")
        .header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json")
        .header(header::ORIGIN,"http://localhost:20001")
        .body(Body::from(json!({"id":offer.id,"grants":[]}).to_string())).unwrap();
    let response=app.oneshot(req).await.unwrap();
    assert_eq!(response.status(),StatusCode::OK);
    let result=json_body(response).await;
    assert_eq!(result["paired"],true);
    assert_eq!(result["role"],"child");
    assert!(result["grants"].as_array().unwrap().is_empty());
    assert_eq!(f.rt.transfer.pending().unwrap()["pending"].as_array().unwrap().len(),0);
    let peers=f.rt.transfer.peers().unwrap();
    assert_eq!(peers["peers"][0]["role"],"child");
    assert_eq!(peers["peers"][0]["grants_revision"].as_str().unwrap().len(),64);
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","read").is_err());
}

#[tokio::test]
async fn child_can_add_project_and_grant_it_after_pairing_without_repair(){
    let f=fixture(|c|{c.transfer.enabled=true;c.workspaces[0].projects.clear();});
    let parent="abcdef123456abcdef123456";
    let token="a".repeat(44);
    let offer=endlessvibe::transfer::secure::Wire{
        kind:"pair_hello".into(),node_id:parent.into(),name:"Linux Parent".into(),
        id:"123456789012345678901234".into(),token:token.clone(),
        tool:String::new(),args:Value::Null
    };
    f.rt.transfer.receive_pair(&offer,"127.0.0.1:23456".parse().unwrap(),"test-certificate").unwrap();
    let app=server::create_dashboard_router(f.rt.clone());
    let request=|method:&str,url:&str,payload:Value|{
        Request::builder().method(method).uri(url).header(header::HOST,"localhost")
            .header(header::CONTENT_TYPE,"application/json")
            .header(header::ORIGIN,"http://localhost:20001")
            .body(Body::from(payload.to_string())).unwrap()
    };
    let approved=app.clone().oneshot(request("POST","/api/nodes/approve",
        json!({"id":offer.id,"grants":[]}))).await.unwrap();
    assert_eq!(approved.status(),StatusCode::OK);
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","read").is_err());

    // A Project can be registered and hot-loaded after the pairing is already complete.
    let project_path=f._dir.path().join("workspace/project");
    let before=endlessvibe::config_edit::revision(&f.rt.config_path).unwrap();
    let added=f.rt.dashboard_add_project(endlessvibe::config_edit::AddProjectRequest{
        expected_revision:before,workspace:"demo".into(),project:Some("demo".into()),
        path:project_path.to_string_lossy().to_string(),
        allow_write:true,allow_exec:false,allow_git_commit:false,
        allow_git_mutation:false,allow_git_push:false,
        execution_profile:None,environment:vec![],
    }).unwrap();
    assert_eq!(added["requires_restart"],false);
    assert!(f.rt.project_exact("demo","demo").is_ok());
    // Registration is not authorization.
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","read").is_err());
    let path=format!("/api/nodes/peers/{parent}/grants");
    let first=f.rt.transfer.peers().unwrap()["peers"][0]["grants_revision"]
        .as_str().unwrap().to_owned();
    let read_only=json!([{"workspace":"demo","project":"demo",
        "read":true,"write":false,"execute":false,"git":false}]);

    // Reject privilege escalation and keep previously empty grants unchanged.
    let elevated=json!([{"workspace":"demo","project":"demo",
        "read":true,"write":false,"execute":true,"git":false}]);
    let denied=app.clone().oneshot(request("PUT",&path,json!({
        "expected_grants_revision":first,"grants":elevated
    }))).await.unwrap();
    assert_eq!(denied.status(),StatusCode::BAD_REQUEST);
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","read").is_err());

    let accepted=app.clone().oneshot(request("PUT",&path,json!({
        "expected_grants_revision":first,"grants":read_only
    }))).await.unwrap();
    assert_eq!(accepted.status(),StatusCode::OK);
    let saved=json_body(accepted).await;
    let current=saved["grants_revision"].as_str().unwrap().to_owned();
    assert_ne!(first,current);
    assert_eq!(saved["grants"][0]["workspace"],"demo");
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","read").is_ok());
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","write").is_err());
    let revived=endlessvibe::transfer::TransferManager::new(
        f.rt.config.transfer.clone(),f.rt.db.clone()).unwrap();
    assert!(revived.authorized(parent,&token,"demo","demo","read").is_ok());

    // Stale edits must not overwrite a grant update.
    let stale=app.clone().oneshot(request("PUT",&path,json!({
        "expected_grants_revision":first,"grants":[]
    }))).await.unwrap();
    assert_eq!(stale.status(),StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["code"],"GRANTS_CONFLICT");
    // Duplicates must not be silently accepted.
    let duplicates=app.clone().oneshot(request("PUT",&path,json!({
        "expected_grants_revision":current,"grants":[read_only[0],read_only[0]]
    }))).await.unwrap();
    assert_eq!(duplicates.status(),StatusCode::BAD_REQUEST);
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","read").is_ok());

    // Removing all grants keeps TLS pairing intact, but revokes Project access immediately.
    let revoked=app.clone().oneshot(request("PUT",&path,json!({
        "expected_grants_revision":current,"grants":[]
    }))).await.unwrap();
    assert_eq!(revoked.status(),StatusCode::OK);
    assert!(json_body(revoked).await["grants"].as_array().unwrap().is_empty());
    assert!(f.rt.transfer.authorized(parent,&token,"demo","demo","read").is_err());
    assert_eq!(f.rt.transfer.peers().unwrap()["peers"].as_array().unwrap().len(),1);

    // Local dashboard CSRF protection remains in effect for grant changes.
    let remote=Request::builder().method("PUT").uri(&path)
        .header(header::HOST,"192.168.1.20")
        .header(header::CONTENT_TYPE,"application/json")
        .header(header::ORIGIN,"http://192.168.1.20:20001")
        .body(Body::from("{}")).unwrap();
    assert_eq!(app.oneshot(remote).await.unwrap().status(),StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn parent_cannot_manage_the_child_side_of_peer_grants(){
    let f=fixture(|c|c.transfer.enabled=true);
    let parent=f.rt.transfer.clone();
    // Simulate an outgoing pairing record by initiating a real TLS handshake
    // with a separate temporary child, then approving the parent side.
    let sock=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=sock.local_addr().unwrap();drop(sock);
    let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});
    let stop=tokio_util::sync::CancellationToken::new();
    let server=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let pending=parent.start_pair(address).await.unwrap();
    // A parent may never approve itself before the child accepts.
    assert!(parent.approve_pair(endlessvibe::transfer::secure::PairApprove{
        id:pending["id"].as_str().unwrap().into(),grants:vec![],
    },&f.rt).is_err());
    child.rt.transfer.approve_pair(endlessvibe::transfer::secure::PairApprove{
        id:pending["id"].as_str().unwrap().into(),grants:vec![],
    },&child.rt).unwrap();
    parent.reconcile_pending().await.unwrap();
    let peers=parent.peers().unwrap();
    let peer=&peers["peers"][0];
    let path=format!("/api/nodes/peers/{}/grants",peer["node_id"].as_str().unwrap());
    let input=json!({"expected_grants_revision":peer["grants_revision"],"grants":[]});
    let app=server::create_dashboard_router(f.rt.clone());
    let request=Request::builder().method("PUT").uri(path)
        .header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json")
        .header(header::ORIGIN,"http://localhost:20001")
        .body(Body::from(input.to_string())).unwrap();
    let response=app.oneshot(request).await.unwrap();
    assert_eq!(response.status(),StatusCode::BAD_REQUEST);
    assert!(json_body(response).await["message"].as_str().unwrap().contains("Only this child's"));
    stop.cancel();server.await.unwrap().unwrap();
}

#[test]fn transfer_pair_grants_do_not_exceed_child_permissions(){let f=fixture(|c|c.transfer.enabled=true);let id="abcdef123456abcdef123456";let secret="a".repeat(44);let offer=endlessvibe::transfer::secure::Wire{kind:"pair_hello".into(),node_id:id.into(),name:"Parent".into(),id:"123456789012345678901234".into(),token:secret.clone(),tool:String::new(),args:Value::Null};let v=f.rt.transfer.receive_pair(&offer,"127.0.0.1:23456".parse().unwrap(),"cert").unwrap();assert_eq!(v["ok"],true);let p=endlessvibe::transfer::secure::PairApprove{id:"123456789012345678901234".into(),grants:vec![endlessvibe::transfer::secure::Grant{workspace:"demo".into(),project:"demo".into(),read:true,write:false,execute:false,git:false}]};assert!(f.rt.transfer.authorized(id,&secret,"demo","demo","read").is_err());assert_eq!(f.rt.transfer.approve_pair(p,&f.rt).unwrap()["paired"],true);assert!(f.rt.transfer.authorized(id,&secret,"demo","demo","read").is_ok());assert!(f.rt.transfer.authorized(id,&secret,"demo","demo","write").is_err());assert!(f.rt.transfer.authorized(id,"wrong-token","demo","demo","read").is_err());assert!(f.rt.transfer.receive_pair(&endlessvibe::transfer::secure::Wire{node_id:"123456abcdef123456abcdef".into(),..offer},"127.0.0.1:23456".parse().unwrap(),"cert").is_err());assert_eq!(f.rt.transfer.revoke_pair(id).unwrap()["revoked"],true);assert!(f.rt.transfer.authorized(id,&secret,"demo","demo","read").is_err());}
#[tokio::test]
async fn paired_tls_nodes_discover_new_project_only_after_child_grants_it() {
    let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=socket.local_addr().unwrap();drop(socket);
    let child=fixture(|c|{
        c.transfer.enabled=true;
        c.transfer.listen=address;
        c.workspaces[0].projects.clear();
    });
    let parent=fixture(|c|c.transfer.enabled=true);
    let stop=tokio_util::sync::CancellationToken::new();
    let tls=tokio::spawn(child.rt.transfer.clone().run_tls(
        stop.clone(),Some(child.rt.clone())
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let offer=parent.rt.transfer.start_pair(address).await.unwrap();
    let id=offer["id"].as_str().unwrap().to_owned();
    let parent_id=parent.rt.transfer.node_id.clone();
    child.rt.transfer.approve_pair(
        endlessvibe::transfer::secure::PairApprove{id:id.clone(),grants:vec![]},
        &child.rt
    ).unwrap();
    parent.rt.transfer.reconcile_pending().await.unwrap();
    let child_id=child.rt.transfer.node_id.clone();
    assert!(parent.rt.transfer.call_node(&child_id,"list_workspaces",json!({}))
        .await.unwrap()["workspaces"].as_array().unwrap().is_empty());
    let args=json!({"workspace":"demo","project":"demo","path":"src/main.rs"});
    assert!(parent.rt.transfer.call_node(&child_id,"read_file",args.clone()).await.is_err());

    // Register the Project after TLS pairing; registration alone is not a grant.
    let project_path=child._dir.path().join("workspace/project");
    let result=child.rt.dashboard_add_project(endlessvibe::config_edit::AddProjectRequest{
        expected_revision:endlessvibe::config_edit::revision(&child.rt.config_path).unwrap(),
        workspace:"demo".into(),project:Some("demo".into()),
        path:project_path.to_string_lossy().to_string(),
        allow_write:false,allow_exec:false,allow_git_commit:false,
        allow_git_mutation:false,allow_git_push:false,
        execution_profile:None,environment:vec![],
    }).unwrap();
    assert_eq!(result["requires_restart"],false);
    assert!(parent.rt.transfer.call_node(&child_id,"list_workspaces",json!({}))
        .await.unwrap()["workspaces"].as_array().unwrap().is_empty());

    let rev=child.rt.transfer.peers().unwrap()["peers"][0]["grants_revision"]
        .as_str().unwrap().to_owned();
    let grant=endlessvibe::transfer::secure::Grant{
        workspace:"demo".into(),project:"demo".into(),
        read:true,write:false,execute:false,git:false,
    };
    child.rt.transfer.update_peer_grants(&parent_id,
        endlessvibe::transfer::secure::GrantUpdate{
            expected_grants_revision:rev,grants:vec![grant],
        },&child.rt
    ).unwrap();
    let workspaces=parent.rt.transfer.call_node(&child_id,"list_workspaces",
        json!({})).await.unwrap();
    assert_eq!(workspaces["workspaces"][0]["id"],"demo");
    let content=parent.rt.transfer.call_node(&child_id,"read_file",args.clone())
        .await.unwrap();
    assert!(content["content"].as_str().unwrap().contains("hello"));

    let rev=child.rt.transfer.peers().unwrap()["peers"][0]["grants_revision"]
        .as_str().unwrap().to_owned();
    child.rt.transfer.update_peer_grants(&parent_id,
        endlessvibe::transfer::secure::GrantUpdate{
            expected_grants_revision:rev,grants:vec![],
        },&child.rt
    ).unwrap();
    assert!(parent.rt.transfer.call_node(&child_id,"read_file",args).await.is_err());
    assert!(parent.rt.transfer.call_node(&child_id,"list_workspaces",
        json!({})).await.unwrap()["workspaces"].as_array().unwrap().is_empty());

    stop.cancel();
    tls.await.unwrap().unwrap();
}
#[tokio::test]async fn transfer_parent_reads_child_over_pinned_tls_and_rejects_unpaired_work(){let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=socket.local_addr().unwrap();drop(socket);let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});let parent=fixture(|c|c.transfer.enabled=true);initialize_git(&child,true);let shutdown=tokio_util::sync::CancellationToken::new();let task=tokio::spawn(child.rt.transfer.clone().run_tls(shutdown.clone(),Some(child.rt.clone())));tokio::time::sleep(Duration::from_millis(100)).await;let pending=parent.rt.transfer.start_pair(address).await.unwrap();let id=pending["id"].as_str().unwrap().to_owned();assert_eq!(child.rt.transfer.pending().unwrap()["pending"][0]["code"],pending["code"]);let grant=endlessvibe::transfer::secure::Grant{workspace:"demo".into(),project:"demo".into(),read:true,write:false,execute:false,git:false};child.rt.transfer.approve_pair(endlessvibe::transfer::secure::PairApprove{id:id.clone(),grants:vec![grant]},&child.rt).unwrap();parent.rt.transfer.reconcile_pending().await.unwrap();let node_id=child.rt.transfer.node_id.clone();let ws=parent.rt.transfer.call_node(&node_id,"list_workspaces",json!({})).await.unwrap();assert_eq!(ws["workspaces"][0]["id"],"demo");let pj=parent.rt.transfer.call_node(&node_id,"list_projects",json!({"workspace":"demo"})).await.unwrap();assert_eq!(pj["projects"][0]["id"],"demo");let content=parent.rt.transfer.call_node(&node_id,"read_file",json!({"workspace":"demo","project":"demo","path":"src/main.rs"})).await.unwrap();assert!(content["content"].as_str().unwrap().contains("hello"));let git=parent.rt.transfer.call_node(&node_id,"git_status",json!({"workspace":"demo","project":"demo"})).await.unwrap();assert_eq!(git["branch"],"main");assert!(parent.rt.transfer.call_node(&node_id,"apply_patch",json!({"workspace":"demo","project":"demo"})).await.is_err());let app=server::create_dashboard_router(parent.rt.clone());let request=Request::builder().method("POST").uri("/api/nodes/read").header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json").header(header::ORIGIN,"http://localhost:20001").body(Body::from(json!({"node_id":node_id,"tool":"read_file","arguments":{"workspace":"demo","project":"demo","path":"src/main.rs"}}).to_string())).unwrap();let response=app.clone().oneshot(request).await.unwrap();assert_eq!(response.status(),StatusCode::OK);shutdown.cancel();task.await.unwrap().unwrap();assert!(parent.rt.transfer.call_node(&node_id,"read_file",json!({"workspace":"demo","project":"demo","path":"src/main.rs"})).await.is_err());}
#[tokio::test]
async fn cached_chatgpt_tools_read_paired_child_via_virtual_project(){
    let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=socket.local_addr().unwrap();drop(socket);
    let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});
    let parent=fixture(|c|{c.transfer.enabled=true;});
    initialize_git(&child,true);
    let stop=tokio_util::sync::CancellationToken::new();
    let tls=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let offer=parent.rt.transfer.start_pair(address).await.unwrap();
    let grant=endlessvibe::transfer::secure::Grant{
        workspace:"demo".into(),project:"demo".into(),read:true,write:false,execute:false,git:false};
    child.rt.transfer.approve_pair(endlessvibe::transfer::secure::PairApprove{
        id:offer["id"].as_str().unwrap().into(),grants:vec![grant]
    },&child.rt).unwrap();
    parent.rt.transfer.reconcile_pending().await.unwrap();
    let node=child.rt.transfer.node_id.clone();
    let alias=format!("node:{node}:demo:demo");
    let app=server::create_router(parent.rt.clone());
    let token=parent.rt.auth.issue_local_token().unwrap();
    async fn call(app:&Router,token:&str,name:&str,args:Value)->Value{
        let message=json!({"jsonrpc":"2.0","id":5,"method":"tools/call",
                          "params":{"name":name,"arguments":args}});
        let response=http(app,"POST","/mcp",Body::from(message.to_string()),
                         Some("application/json"),Some(token),None).await;
        assert_eq!(response.status(),StatusCode::OK);
        json_body(response).await["result"].clone()
    }
    let projects=call(&app,&token,"list_projects",json!({})).await;
    assert_ne!(projects["isError"],true,"{projects}");
    let projects=projects["structuredContent"]["projects"].as_array().unwrap();
    let selected=projects.iter().find(|p|p["id"]==alias).expect("child project missing from cached list_projects");
    assert_eq!(selected["remote"],true);
    assert_eq!(selected["remote_workspace"],"demo");
    let read=call(&app,&token,"read_file",json!({"workspace":alias,"path":"src/main.rs"})).await;
    assert_ne!(read["isError"],true,"{read}");
    assert!(read["structuredContent"]["content"].as_str().unwrap().contains("hello"));
    let status=call(&app,&token,"git_status",json!({"workspace":alias})).await;
    assert_ne!(status["isError"],true,"{status}");
    assert_eq!(status["structuredContent"]["branch"],"main");
    let denied=call(&app,&token,"read_file",json!({
        "workspace":format!("node:{node}:demo:../escape"),"path":"src/main.rs"
    })).await;
    assert_eq!(denied["isError"],true);
    let rev=child.rt.transfer.peers().unwrap()["peers"][0]["grants_revision"].as_str().unwrap().to_owned();
    child.rt.transfer.update_peer_grants(&parent.rt.transfer.node_id,
        endlessvibe::transfer::secure::GrantUpdate{expected_grants_revision:rev,grants:vec![]},
        &child.rt).unwrap();
    let after=call(&app,&token,"read_file",json!({"workspace":alias,"path":"src/main.rs"})).await;
    assert_eq!(after["isError"],true,"Revocation must prevent cached virtual project access");
    let local=call(&app,&token,"read_file",json!({"workspace":"demo","path":"src/main.rs"})).await;
    assert_ne!(local["isError"],true,"Local legacy project must keep working");
    stop.cancel();tls.await.unwrap().unwrap();
}

#[tokio::test]
async fn cached_chatgpt_tools_commit_and_poll_paired_child(){
    let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=socket.local_addr().unwrap();drop(socket);
    let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});
    let parent=fixture(|c|{c.transfer.enabled=true;});
    initialize_git(&child,true);
    let stop=tokio_util::sync::CancellationToken::new();
    let tls=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let offer=parent.rt.transfer.start_pair(address).await.unwrap();
    let grant=endlessvibe::transfer::secure::Grant{
        workspace:"demo".into(),project:"demo".into(),read:true,write:true,execute:true,git:true};
    child.rt.transfer.approve_pair(endlessvibe::transfer::secure::PairApprove{
        id:offer["id"].as_str().unwrap().into(),grants:vec![grant]
    },&child.rt).unwrap();
    parent.rt.transfer.reconcile_pending().await.unwrap();
    let alias=format!("node:{}:demo:demo",child.rt.transfer.node_id);
    let app=server::create_router(parent.rt.clone());
    let token=parent.rt.auth.issue_local_token().unwrap();
    async fn call(app:&Router,token:&str,name:&str,args:Value)->Value{
        let message=json!({"jsonrpc":"2.0","id":6,"method":"tools/call",
                           "params":{"name":name,"arguments":args}});
        let response=http(app,"POST","/mcp",Body::from(message.to_string()),
                         Some("application/json"),Some(token),None).await;
        assert_eq!(response.status(),StatusCode::OK);
        json_body(response).await["result"].clone()
    }
    let original=call(&app,&token,"read_file",json!({"workspace":alias,"path":"tracked.txt"})).await;
    assert_ne!(original["isError"],true,"{original}");
    let hash=original["structuredContent"]["sha256"].as_str().unwrap();
    let change=json!({"workspace":alias,"path":"tracked.txt","content":"changed from cached client\n",
                     "expected_sha256":hash});
    let written=call(&app,&token,"write_file",change.clone()).await;
    assert_ne!(written["isError"],true,"{written}");
    let request_id=written["structuredContent"]["remote_request_id"].as_str().unwrap();
    assert!(request_id.starts_with("legacy_"));
    let replay=call(&app,&token,"write_file",change).await;
    assert_ne!(replay["isError"],true,"Idempotent re-request must reuse first result: {replay}");
    assert_eq!(written["structuredContent"]["sha256"],replay["structuredContent"]["sha256"]);
    let diff=call(&app,&token,"git_diff",json!({"workspace":alias,"paths":["tracked.txt"]})).await;
    assert_ne!(diff["isError"],true,"{diff}");
    let review=&diff["structuredContent"];
    assert_eq!(review["has_more"],false);
    let commit=call(&app,&token,"git_commit",json!({
        "workspace":alias,"paths":["tracked.txt"],"message":"fix(test): verify cached child commit",
        "expected_head":review["head"],"expected_diff_sha256":review["diff_sha256"]
    })).await;
    assert_ne!(commit["isError"],true,"{commit}");
    let committed_head=commit["structuredContent"]["commit"].as_str().unwrap();
    let status=call(&app,&token,"git_status",json!({"workspace":alias})).await;
    assert_ne!(status["isError"],true,"{status}");
    assert_eq!(status["structuredContent"]["head"],committed_head);
    let history=call(&app,&token,"git_log",json!({"workspace":alias,"limit":1})).await;
    assert_ne!(history["isError"],true,"{history}");
    assert_eq!(history["structuredContent"]["commits"][0]["subject"],"fix(test): verify cached child commit");
    let command=call(&app,&token,"run_command",json!({
        "workspace":alias,"program":"git","args":["--version"],
        "request_id":"cached-child-git-version","timeout_seconds":20
    })).await;
    assert_ne!(command["isError"],true,"{command}");
    let handle=command["structuredContent"]["job_id"].as_str().unwrap().to_owned();
    assert!(handle.starts_with("nodejob:"),"remote jobs must have a routable handle");
    let mut state=Value::Null;
    for _ in 0..50{
        let result=call(&app,&token,"get_job",json!({"job_id":handle})).await;
        assert_ne!(result["isError"],true,"{result}");
        state=result["structuredContent"].clone();
        if state["status"]=="succeeded"{break;}
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert_eq!(state["status"],"succeeded");
    let output=call(&app,&token,"get_job_output",json!({"job_id":handle,"limit":4096})).await;
    assert_ne!(output["isError"],true,"{output}");
    assert!(output["structuredContent"]["output"].as_str().unwrap().contains("git version"));
    let op:i64=parent.rt.db.transaction(|tx|Ok(tx.query_row(
        "SELECT MAX(seq) FROM operation_log WHERE tool='node_write'",[],|r|r.get(0))?)).unwrap();
    let stored=parent.rt.db.operation(op).unwrap();
    assert_eq!(stored["output"]["redacted"],true);
    assert!(!stored.to_string().contains("changed from cached client"));
    let revision=child.rt.transfer.peers().unwrap()["peers"][0]["grants_revision"].as_str().unwrap().to_owned();
    child.rt.transfer.update_peer_grants(&parent.rt.transfer.node_id,
        endlessvibe::transfer::secure::GrantUpdate{
            expected_grants_revision:revision,
            grants:vec![endlessvibe::transfer::secure::Grant{
                workspace:"demo".into(),project:"demo".into(),read:true,
                write:false,execute:false,git:false
            }],
        },&child.rt).unwrap();
    let denied=call(&app,&token,"create_directory",json!({"workspace":alias,"path":"denied"})).await;
    assert_eq!(denied["isError"],true,"Child grant revocation must block cached clients");
    assert!(!child.rt.project("demo","demo").unwrap().root.path.join("denied").exists());
    stop.cancel();tls.await.unwrap().unwrap();
}

#[tokio::test]async fn transfer_parent_mutates_and_polls_child_jobs_over_tls(){
let sock=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=sock.local_addr().unwrap();drop(sock);
let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});let parent=fixture(|c|c.transfer.enabled=true);initialize_git(&child,true);
let stop=tokio_util::sync::CancellationToken::new();let tls=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));tokio::time::sleep(Duration::from_millis(100)).await;
let offer=parent.rt.transfer.start_pair(address).await.unwrap();let id=offer["id"].as_str().unwrap().to_owned();
let grant=endlessvibe::transfer::secure::Grant{workspace:"demo".into(),project:"demo".into(),read:true,write:true,execute:true,git:true};
child.rt.transfer.approve_pair(endlessvibe::transfer::secure::PairApprove{id:id.clone(),grants:vec![grant]},&child.rt).unwrap();
parent.rt.transfer.reconcile_pending().await.unwrap();
let node=child.rt.transfer.node_id.clone();let w=json!({"workspace":"demo","project":"demo","path":"src/main.rs"});
let original=parent.rt.transfer.call_node(&node,"read_file",w.clone()).await.unwrap();
let h=original["sha256"].as_str().unwrap();
let patch=json!({"workspace":"demo","project":"demo","path":"src/main.rs","expected_sha256":h,"edits":[{"old_text":"hello","new_text":"transferred","expected_occurrences":1}]});
let written=parent.rt.transfer.call_node_with_request_id(&node,"apply_patch",patch.clone(),Some("integration-patch")).await.unwrap();
let state=parent.rt.transfer.call_node(&node,"request_status",json!({"workspace":"demo","project":"demo","request_id":"integration-patch"})).await.unwrap();assert_eq!(state["state"],"completed");
assert_eq!(parent.rt.transfer.call_node_with_request_id(&node,"apply_patch",patch,Some("integration-patch")).await.unwrap()["changed"],true);
assert!(parent.rt.transfer.call_node(&node,"request_status",json!({"workspace":"demo","project":"other","request_id":"integration-patch"})).await.is_err());
assert_eq!(written["changed"],true);assert!(parent.rt.transfer.call_node(&node,"read_file",w.clone()).await.unwrap()["content"].as_str().unwrap().contains("transferred"));
assert!(parent.rt.transfer.call_node(&node,"apply_patch",json!({"workspace":"demo","project":"demo","path":"src/main.rs","expected_sha256":h,"edits":[{"old_text":"hello","new_text":"stale"}]})).await.is_err());
let job=parent.rt.transfer.call_node(&node,"run_command",json!({"workspace":"demo","project":"demo","program":"git","args":["--version"],"request_id":"transfer-test-version","timeout_seconds":20})).await.unwrap();
let job_id=job["job_id"].as_str().unwrap().to_owned();
for _ in 0..80{let status=parent.rt.transfer.call_node(&node,"get_job",json!({"workspace":"demo","project":"demo","job_id":job_id})).await.unwrap();if status["status"]=="succeeded"{break;}tokio::time::sleep(Duration::from_millis(40)).await;}
let status=parent.rt.transfer.call_node(&node,"get_job",json!({"workspace":"demo","project":"demo","job_id":job_id})).await.unwrap();assert_eq!(status["status"],"succeeded");
let recent=parent.rt.transfer.call_node(&node,"request_history",json!({"workspace":"demo","project":"demo","limit":20})).await.unwrap();
let linked=recent["requests"].as_array().unwrap().iter().find(|r|r["request_id"]=="transfer-test-version").unwrap();
assert_eq!(linked["state"],"completed");
assert_eq!(linked["job_status"],"succeeded");
assert_eq!(linked["job_id"],job_id);
let output=parent.rt.transfer.call_node(&node,"get_job_output",json!({"workspace":"demo","project":"demo","job_id":job_id,"limit":2048})).await.unwrap();assert!(output["output"].as_str().unwrap().contains("git version"));
let task_id=job["task_id"].as_str().unwrap();let checkpoint=parent.rt.transfer.call_node(&node,"get_task_checkpoint",json!({"workspace":"demo","project":"demo","task_id":task_id})).await.unwrap();assert_eq!(checkpoint["latest"]["status"],"succeeded");let recovery=parent.rt.transfer.call_node(&node,"continue_task",json!({"workspace":"demo","project":"demo","task_id":task_id})).await.unwrap();assert_eq!(recovery["resolved_task_id"],task_id);
let history=parent.rt.transfer.call_node(&node,"list_task_checkpoints",json!({"workspace":"demo","project":"demo","task_id":task_id,"limit":5})).await.unwrap();assert_eq!(history["checkpoints"].as_array().unwrap().len(),1);
assert!(parent.rt.transfer.call_node(&node,"list_task_checkpoints",json!({"workspace":"other","project":"demo","task_id":task_id,"limit":5})).await.is_err());
let replay=parent.rt.transfer.call_node(&node,"run_command",json!({"workspace":"demo","project":"demo","program":"git","args":["--version"],"request_id":"transfer-test-version","timeout_seconds":20})).await.unwrap();assert_eq!(replay["reused"],true);assert_eq!(replay["job_id"],job_id);
assert!(parent.rt.transfer.call_node(&node,"get_job",json!({"workspace":"demo","project":"demo","job_id":"not-the-same"})).await.is_err());
assert!(parent.rt.transfer.call_node(&node,"run_shell",json!({"workspace":"demo","project":"demo","script":"echo bad"})).await.is_err());
stop.cancel();tls.await.unwrap().unwrap();
assert!(parent.rt.transfer.call_node(&node,"get_job",json!({"workspace":"demo","project":"demo","job_id":job_id})).await.is_err());
let recovered=endlessvibe::transfer::TransferManager::new(child.rt.config.transfer.clone(),child.rt.db.clone()).unwrap();assert_eq!(recovered.node_id,node);assert_eq!(recovered.peers().unwrap()["peers"].as_array().unwrap().len(),1);
let resume=tokio_util::sync::CancellationToken::new();let running=tokio::spawn(recovered.clone().run_tls(resume.clone(),Some(child.rt.clone())));tokio::time::sleep(Duration::from_millis(100)).await;
let old=parent.rt.transfer.call_node(&node,"get_job",json!({"workspace":"demo","project":"demo","job_id":job_id})).await.unwrap();assert_eq!(old["status"],"succeeded");
child.rt.transfer.revoke_pair(&parent.rt.transfer.node_id).unwrap();assert!(parent.rt.transfer.call_node(&node,"get_job",json!({"workspace":"demo","project":"demo","job_id":job_id})).await.is_err());
resume.cancel();running.await.unwrap().unwrap();
}

#[tokio::test]
async fn transfer_request_history_survives_runtime_restart_and_revocation(){
 let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
 let address=socket.local_addr().unwrap();drop(socket);
 let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});
 let parent=fixture(|c|c.transfer.enabled=true);
 let parent_id=parent.rt.transfer.node_id.clone();
 let node_id=child.rt.transfer.node_id.clone();
 let stop=tokio_util::sync::CancellationToken::new();
 let server_task=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));
 tokio::time::sleep(Duration::from_millis(100)).await;
 let offer=parent.rt.transfer.start_pair(address).await.unwrap();
 let pair_id=offer["id"].as_str().unwrap().to_string();
 let grant=endlessvibe::transfer::secure::Grant{workspace:"demo".into(),project:"demo".into(),read:true,write:true,execute:false,git:false};
 child.rt.transfer.approve_pair(endlessvibe::transfer::secure::PairApprove{id:pair_id.clone(),grants:vec![grant]},&child.rt).unwrap();
 parent.rt.transfer.reconcile_pending().await.unwrap();
 let good=json!({"workspace":"demo","project":"demo","path":"restored-directory"});
 parent.rt.transfer.call_node_with_request_id(&node_id,"create_directory",good.clone(),Some("confirmed-create")).await.unwrap();
 let bad=json!({"workspace":"demo","project":"demo","path":"../outside-project"});
 assert!(parent.rt.transfer.call_node_with_request_id(&node_id,"create_directory",bad.clone(),Some("rejected-create")).await.is_err());
 let failed=parent.rt.transfer.call_node(&node_id,"request_status",json!({"workspace":"demo","project":"demo","request_id":"rejected-create"})).await.unwrap();
 assert_eq!(failed["state"],"failed");

 // Simulate the durable record left by a process killed after claiming a write,
 // then actually stop the TLS listener and recreate the whole Runtime/Store.
 let interrupted_id="crash-during-write";
 let interrupted_args=json!({"workspace":"demo","project":"demo","path":"recovery.txt","expected_sha256":"MISSING","content":"sensitive payload"});
 let digest=util::digest(format!("write_file\0{}",interrupted_args));
 let kv_key=util::digest(format!("{parent_id}\0{interrupted_id}"));
 let now=util::now();
 let record=json!({"fingerprint":digest,"state":"started","result":null,
  "workspace":"demo","project":"demo","tool":"write_file",
  "error":null,"job_id":null,"request_id":interrupted_id,"peer_node_id":parent_id,
  "created":now,"updated":now});
 child.rt.db.put("transfer_requests",&kv_key,&record,0).unwrap();
 let unknown=parent.rt.transfer.call_node(&node_id,"request_status",json!({"workspace":"demo","project":"demo","request_id":interrupted_id})).await.unwrap();
 assert_eq!(unknown["state"],"uncertain");
 let config=(*child.rt.config).clone();
 let config_path=child.rt.config_path.clone();
 stop.cancel();server_task.await.unwrap().unwrap();
 drop(child.rt);
 // An accepted TLS session may finish releasing its Runtime Arc just after
 // the listener task exits; wait only for that known transient file lock.
 let restarted=tokio::time::timeout(Duration::from_secs(3),async{
  loop{
   match Runtime::new(config.clone(),&config_path){
    Ok(rt)=>break rt,
    Err(error) if error.to_string().contains("Another EndlessVibe process is using this state directory")=>{
     tokio::time::sleep(Duration::from_millis(20)).await;
    },
    Err(error)=>panic!("Transfer Runtime restart failed: {error:#}"),
   }
  }
 }).await.expect("Active TLS sessions did not release the state lock after shutdown");
 assert_eq!(restarted.transfer.node_id,node_id);
 let resume=tokio_util::sync::CancellationToken::new();
 let restart_task=tokio::spawn(restarted.transfer.clone().run_tls(resume.clone(),Some(restarted.clone())));
 tokio::time::sleep(Duration::from_millis(100)).await;
 let state=parent.rt.transfer.call_node(&node_id,"request_status",json!({"workspace":"demo","project":"demo","request_id":interrupted_id})).await.unwrap();
 assert_eq!(state["state"],"interrupted");
 assert_eq!(state["safe_to_replay"],false);
 assert_eq!(state["recovery_action"],"inspect_project_before_new_request");
 let history=parent.rt.transfer.call_node(&node_id,"request_history",json!({"workspace":"demo","project":"demo","limit":20})).await.unwrap();
 let rows=history["requests"].as_array().unwrap();
 let first_page=parent.rt.transfer.call_node(&node_id,"request_history",json!({"workspace":"demo","project":"demo","limit":1})).await.unwrap();
 assert_eq!(first_page["requests"].as_array().unwrap().len(),1);
 assert_eq!(first_page["has_more"],true);
 let cursor=first_page["next_cursor"].as_str().unwrap();
 let second_page=parent.rt.transfer.call_node(&node_id,"request_history",json!({"workspace":"demo","project":"demo","limit":1,"cursor":cursor})).await.unwrap();
 assert_ne!(first_page["requests"][0]["request_id"],second_page["requests"][0]["request_id"]);
 assert!(parent.rt.transfer.call_node(&node_id,"request_history",json!({"workspace":"demo","project":"demo","limit":1,"cursor":"invalid"})).await.is_err());
 assert!(rows.iter().any(|r|r["request_id"]=="confirmed-create"&&r["state"]=="completed"));
 assert!(rows.iter().any(|r|r["request_id"]=="rejected-create"&&r["state"]=="failed"));
 assert!(rows.iter().any(|r|r["request_id"]==interrupted_id&&r["state"]=="interrupted"));
 assert!(parent.rt.transfer.call_node_with_request_id(&node_id,"write_file",interrupted_args,Some(interrupted_id)).await.is_err());
 assert!(!restarted.project("demo","demo").unwrap().root.path.join("recovery.txt").exists());
 parent.rt.transfer.call_node_with_request_id(&node_id,"create_directory",good,Some("confirmed-create")).await.unwrap();
 restarted.transfer.revoke_pair(&parent_id).unwrap();
 assert!(parent.rt.transfer.call_node(&node_id,"request_history",json!({"workspace":"demo","project":"demo"})).await.is_err());
 resume.cancel();restart_task.await.unwrap().unwrap();
}
#[tokio::test]
async fn transfer_offline_and_lost_ack_do_not_replay_mutations(){
    let socket=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=socket.local_addr().unwrap();drop(socket);
    let child=fixture(|c|{c.transfer.enabled=true;c.transfer.listen=address;});
    let parent=fixture(|c|c.transfer.enabled=true);
    let stop=tokio_util::sync::CancellationToken::new();
    let server=tokio::spawn(child.rt.transfer.clone().run_tls(stop.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let offer=parent.rt.transfer.start_pair(address).await.unwrap();
    let pair_id=offer["id"].as_str().unwrap().to_owned();
    let grant=endlessvibe::transfer::secure::Grant{workspace:"demo".into(),project:"demo".into(),read:true,write:true,execute:false,git:false};
    child.rt.transfer.approve_pair(endlessvibe::transfer::secure::PairApprove{id:pair_id.clone(),grants:vec![grant]},&child.rt).unwrap();
    parent.rt.transfer.reconcile_pending().await.unwrap();
    let node=child.rt.transfer.node_id.clone();
    let workspace=child.rt.project("demo","demo").unwrap().root.path.clone();
    stop.cancel();server.await.unwrap().unwrap();
    let undelivered=json!({"workspace":"demo","project":"demo","path":"not-yet-delivered"});
    assert!(parent.rt.transfer.call_node_with_request_id(&node,"create_directory",undelivered.clone(),Some("offline-delivery")).await.is_err());
    assert!(!workspace.join("not-yet-delivered").exists());
    let key=util::digest(format!("{}\0offline-delivery",parent.rt.transfer.node_id));
    assert!(child.rt.db.get::<Value>("transfer_requests",&key).unwrap().is_none());
    let resume=tokio_util::sync::CancellationToken::new();
    let running=tokio::spawn(child.rt.transfer.clone().run_tls(resume.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;
    parent.rt.transfer.call_node_with_request_id(&node,"create_directory",undelivered,Some("offline-delivery")).await.unwrap();
    assert!(workspace.join("not-yet-delivered").is_dir());
    let delivered=json!({"workspace":"demo","project":"demo","path":"already-delivered"});
    // Emulate an application losing the response after the child commits the side effect.
    let acknowledged=parent.rt.transfer.call_node_with_request_id(&node,"create_directory",delivered.clone(),Some("lost-ack")).await.unwrap();
    let response_digest=util::digest(acknowledged.to_string());drop(acknowledged);
    assert!(workspace.join("already-delivered").is_dir());
    let before:i64=child.rt.db.transaction(|tx|Ok(tx.query_row("SELECT COUNT(*) FROM operation_log WHERE tool='transfer_create_directory'",[],|r|r.get(0))?)).unwrap();
    assert_eq!(before,2);
    resume.cancel();running.await.unwrap().unwrap();
    let reboot=tokio_util::sync::CancellationToken::new();
    let rebooted=tokio::spawn(child.rt.transfer.clone().run_tls(reboot.clone(),Some(child.rt.clone())));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let replay=parent.rt.transfer.call_node_with_request_id(&node,"create_directory",delivered,Some("lost-ack")).await.unwrap();
    assert_eq!(util::digest(replay.to_string()),response_digest);
    let after:i64=child.rt.db.transaction(|tx|Ok(tx.query_row("SELECT COUNT(*) FROM operation_log WHERE tool='transfer_create_directory'",[],|r|r.get(0))?)).unwrap();
    assert_eq!(after,before);
    let status=parent.rt.transfer.call_node(&node,"request_status",json!({"workspace":"demo","project":"demo","request_id":"lost-ack"})).await.unwrap();
    assert_eq!(status["state"],"completed");
    assert_eq!(status["safe_to_replay"],false);
    reboot.cancel();rebooted.await.unwrap().unwrap();
}
#[test]fn transfer_operation_audit_never_persists_remote_output_or_diff(){let f=fixture(|_|{});for tool in ["node_read","node_write","node_dashboard_write","transfer_get_job_output","transfer_git_diff"]{let op=f.rt.begin_operation(tool,"demo","demo",json!({"request_hash":"digest"}));let id=op.id.unwrap();let delivered=f.rt.finish_operation(op,Ok(json!({"output":"REMOTE_SECRET_LOG","diff":"REMOTE_SECRET_PATCH"}))).unwrap();assert_eq!(delivered["output"],"REMOTE_SECRET_LOG");let stored=f.rt.db.operation(id).unwrap();assert_eq!(stored["output"]["redacted"],true);assert!(!stored.to_string().contains("REMOTE_SECRET_LOG"));assert!(!stored.to_string().contains("REMOTE_SECRET_PATCH"));}}
#[tokio::test]
async fn dashboard_theme_language_and_bilingual_readmes_are_available(){
 let f=fixture(|_|{});
 let router=server::create_dashboard_router(f.rt.clone());
 for page in ["/","/projects","/tasks","/activity","/operations","/mcp","/config","/nodes"]{
  let response=http(&router,"GET",page,Body::empty(),None,None,None).await;
  assert_eq!(response.status(),StatusCode::OK,"{page}");
  let body=to_bytes(response.into_body(),100_000).await.unwrap();
  let html=String::from_utf8_lossy(&body);
  for marker in ["id=\"theme-select\"","id=\"language-select\"",
     "/assets/js/preferences-init.js","/assets/theme.css","/assets/app.js"]{
    assert!(html.contains(marker),"{page} missing {marker}");
  }
 }
 for (path,marker) in [
   ("/assets/theme.css","html[data-theme=\"light\"]"),
   ("/assets/js/preferences-init.js","endlessvibe.theme"),
   ("/assets/js/preferences.js","initPreferences"),
   ("/assets/js/preferences.js","MutationObserver"),
   ("/assets/js/charts.js","refreshChartTheme"),
 ]{
  let response=http(&router,"GET",path,Body::empty(),None,None,None).await;
  assert_eq!(response.status(),StatusCode::OK,"{path}");
  let mime=response.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap().to_owned();
  assert!(mime.contains(if path.ends_with(".css"){"text/css"}else{"text/javascript"}));
  let bytes=to_bytes(response.into_body(),150_000).await.unwrap();
  assert!(String::from_utf8_lossy(&bytes).contains(marker),"{path} missing {marker}");
 }
 let zh=include_str!("../README.md");
 let en=include_str!("../README.en.md");
 assert!(zh.contains("README.en.md"));
 assert!(en.contains("README.md"));
 assert!(en.contains("## Quick Start"));
 for filename in ["setup","setup-workspace","setup-project","setup-finish","dashboard","projects","tasks","operations","nodes"]{
  let zh_image=format!("docs/screenshots/{filename}.jpg");
  let en_image=format!("docs/screenshots/{filename}-en-light.jpg");
  assert!(zh.contains(&zh_image),"Chinese README missing {zh_image}");
  assert!(en.contains(&en_image),"English README missing {en_image}");
  for image in [&zh_image,&en_image]{
   assert!(std::path::Path::new(image).exists(),"missing screenshot: {image}");
  }
 }
}

#[tokio::test]
async fn nodes_request_history_ui_assets_are_served(){
 let f=fixture(|_|{});
 let router=server::create_dashboard_router(f.rt.clone());
 for (path,expected) in [
  ("/nodes","id=\"node-request-history\""),
  ("/assets/js/nodes.js","loadRequestHistory"),
  ("/assets/js/nodes.js","request_history"),
  ("/assets/js/nodes.js","Load older requests"),
  ("/nodes","id=\"node-metrics\""),
  ("/assets/js/nodes.js","renderTransferMetrics"),
  ("/assets/js/nodes.js","/api/nodes/metrics"),
  ("/assets/app.css",".node-metrics article"),
  ("/assets/app.css",".node-history-state[data-state=\"interrupted\"]"),
 ]{
  let response=http(&router,"GET",path,Body::empty(),None,None,None).await;
  assert_eq!(response.status(),StatusCode::OK,"{path}");
  let body=to_bytes(response.into_body(),2*1024*1024).await.unwrap();
  assert!(String::from_utf8_lossy(&body).contains(expected),"{path} missing {expected}");
 }
}

#[tokio::test]
async fn remote_file_browser_assets_are_served(){
 let f=fixture(|_|{});
 let router=server::create_dashboard_router(f.rt.clone());
 for (path,needles) in [
  ("/nodes",vec!["id=\"node-remote-browser\"","id=\"node-remote-file-list\"","id=\"node-remote-file-more\""]),
  ("/assets/js/nodes.js",vec!["async function listRemoteFiles","async function readRemoteFile","childEntryPath(path,entry.name)","await listRemoteFiles(peer,w,p,\".\")"]),
  ("/assets/app.css",vec![".node-file-browser{",".node-file-entry{"]),
 ]{
  let response=http(&router,"GET",path,Body::empty(),None,None,None).await;
  assert_eq!(response.status(),StatusCode::OK,"{path}");
  let body=to_bytes(response.into_body(),2*1024*1024).await.unwrap();
  let text=String::from_utf8_lossy(&body);
  for needle in needles{assert!(text.contains(needle),"{path} missing {needle}");}
 }
}

#[tokio::test]
async fn transfer_metrics_are_local_only_and_aggregate(){
 let child=fixture(|c|c.transfer.enabled=true);
 child.rt.db.put("transfer_requests","secret-record",
     &json!({"state":"failed","fingerprint":"PRIVATE_CREDENTIAL","result":null}),0).unwrap();
 let app=server::create_dashboard_router(child.rt.clone());
 let response=http(&app,"GET","/api/nodes/metrics",Body::empty(),None,None,None).await;
 assert_eq!(response.status(),StatusCode::OK);
 let metrics=json_body(response).await;
 assert_eq!(metrics["request_records"]["review_required"],1);
 assert_eq!(metrics["request_records"]["failed"],1);
 assert_eq!(metrics["contains_request_contents"],false);
 assert!(!metrics.to_string().contains("PRIVATE_CREDENTIAL"));
 let public=server::create_router(child.rt.clone());
 let request=Request::builder().uri("/api/nodes/metrics").header(header::HOST,"localhost").body(Body::empty()).unwrap();
 assert_eq!(public.oneshot(request).await.unwrap().status(),StatusCode::NOT_FOUND);
}
#[tokio::test]async fn nodes_dashboard_and_transfer_config_endpoint_are_local_only(){let f=fixture(|_|{});let app=server::create_dashboard_router(f.rt.clone());for(path,text)in[("/nodes","id=\"node-pending-list\""),("/assets/js/nodes.js","renderTrusted")]{let response=http(&app,"GET",path,Body::empty(),None,None,None).await;assert_eq!(response.status(),StatusCode::OK);let bytes=to_bytes(response.into_body(),2*1024*1024).await.unwrap();assert!(String::from_utf8_lossy(&bytes).contains(text));}let old=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;let request=Request::builder().method("PUT").uri("/api/config/transfer").header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001").header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"expected_revision":old["revision"],"enabled":true,"listen":"0.0.0.0:20002","advertise":true,"discover":true,"display_name":"Child Test"}).to_string())).unwrap();let response=app.clone().oneshot(request).await.unwrap();assert_eq!(response.status(),StatusCode::OK);assert_eq!(json_body(response).await["requires_restart"],true);let config=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;assert_eq!(config["transfer"]["display_name"],"Child Test");let denied=Request::builder().method("POST").uri("/api/nodes/read").header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001").header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"node_id":"123456789012345678901234","tool":"write_file","arguments":{}}).to_string())).unwrap();assert_eq!(app.clone().oneshot(denied).await.unwrap().status(),StatusCode::FORBIDDEN);let missing_confirm=Request::builder().method("POST").uri("/api/nodes/write").header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001").header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"node_id":"123456789012345678901234","tool":"run_command","arguments":{"workspace":"demo","project":"demo"},"confirm":false}).to_string())).unwrap();assert_eq!(app.clone().oneshot(missing_confirm).await.unwrap().status(),StatusCode::FORBIDDEN);let forbidden=Request::builder().method("POST").uri("/api/nodes/write").header(header::HOST,"localhost").header(header::ORIGIN,"http://localhost:20001").header(header::CONTENT_TYPE,"application/json").body(Body::from(json!({"node_id":"123456789012345678901234","tool":"run_shell","arguments":{},"confirm":true}).to_string())).unwrap();assert_eq!(app.clone().oneshot(forbidden).await.unwrap().status(),StatusCode::FORBIDDEN);}
struct Fixture{_dir:tempfile::TempDir,rt:Arc<Runtime>,owner:String}
fn fixture(edit:impl FnOnce(&mut Config))->Fixture{
    let d=tempfile::tempdir().unwrap();let root=d.path().join("workspace");let project=root.join("project");std::fs::create_dir_all(project.join("src")).unwrap();std::fs::write(project.join("src/main.rs"),"fn main() {\n    println!(\"hello\");\n}\n").unwrap();
    let mut config=Config::default();config.security.data_dir=d.path().join("state");config.workspaces=vec![WorkspaceConfig{id:"demo".into(),path:root,projects:vec![ProjectConfig{id:"demo".into(),path:"project".into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:endlessvibe::config::default_project_profile(),environment:vec![]}],allow_write:None,allow_exec:None,allow_git_commit:None,allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];config.execution.backend="host".into();config.execution.acknowledge_unsafe_host_execution=true;config.execution.allow_shell=true;config.git.author_name="Test Agent".into();config.git.author_email="test@example.invalid".into();edit(&mut config);
    util::private_dir(&config.security.data_dir).unwrap();let owner=util::random_secret().unwrap();util::private_create(&config.security.data_dir.join("owner.key"),owner.as_bytes()).unwrap();let path=d.path().join("config.toml");util::private_create(&path,toml::to_string(&config).unwrap().as_bytes()).unwrap();let rt=Runtime::new(config,&path).unwrap();Fixture{_dir:d,rt,owner}
}
fn git_cli(path:&Path,args:&[&str])->Vec<u8>{
    // Use the platform's configured Git while keeping every invocation isolated
    // from the developer's global config, hooks, helpers and signing settings.
    let config=Config::default();
    let isolated=tempfile::tempdir().unwrap();
    let empty_config=isolated.path().join("gitconfig");
    let hooks=isolated.path().join("hooks");
    std::fs::write(&empty_config,b"").unwrap();
    std::fs::create_dir(&hooks).unwrap();
    let mut command=std::process::Command::new(&config.git.executable);
    command.current_dir(path).env_clear()
        .env("PATH",&config.execution.path)
        .env("HOME",isolated.path()).env("XDG_CONFIG_HOME",isolated.path())
        .env("USERPROFILE",isolated.path()).env("TEMP",isolated.path()).env("TMP",isolated.path())
        .env("LANG","C.UTF-8").env("LC_ALL","C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL",&empty_config)
        .env("GIT_TERMINAL_PROMPT","0");
    #[cfg(windows)]
    for key in ["SystemRoot","WINDIR","SystemDrive","ComSpec"]{
        if let Some(value)=std::env::var_os(key){command.env(key,value);}
    }
    let out=command.args(["-c","user.name=Test","-c","user.email=test@example.invalid",
        "-c","core.fsmonitor=false","-c","commit.gpgSign=false","-c","core.autocrlf=false"])
        .arg("-c").arg(format!("core.hooksPath={}",hooks.display()))
        .args(args).output().unwrap();
    assert!(out.status.success(),"git {args:?}: {}",String::from_utf8_lossy(&out.stderr));
    out.stdout
}
fn initialize_git(f:&Fixture,initial:bool){let w=f.rt.project("demo","demo").unwrap();git_cli(&w.root.path,&["init","-q","--initial-branch=main"]);if initial{std::fs::write(w.root.path.join("tracked.txt"),"one\n").unwrap();std::fs::write(w.root.path.join("other.txt"),"base\n").unwrap();git_cli(&w.root.path,&["add","--all"]);git_cli(&w.root.path,&["commit","-qm","initial"]);}}

#[cfg(windows)]
#[tokio::test]
async fn windows_git_tools_accept_verbatim_project_and_temporary_paths(){
    let f=fixture(|_|{});
    let project=f.rt.project("demo","demo").unwrap();
    assert!(project.root.path.to_string_lossy().starts_with(r"\\?\"),
            "Windows regression must use a verbatim canonical project path");
    let init=std::process::Command::new(&f.rt.config.git.executable)
        .current_dir(&project.root.path)
        .args(["init","--quiet","--initial-branch=dev"])
        .output().unwrap();
    assert!(init.status.success(),"{}",String::from_utf8_lossy(&init.stderr));

    // Exercises --file, --git-dir and --work-tree using the secure preflight.
    let status=git::status(&f.rt,&project).await.unwrap();
    assert_eq!(status["branch"],"dev");
    assert_eq!(status["head"],"UNBORN");
    let log=git::log(&f.rt,&project,LogArgs{
        workspace:"demo".into(),project:"demo".into(),limit:2,task_id:None,stage:None
    }).await.unwrap();
    assert!(log["commits"].as_array().unwrap().is_empty());

    // Exercises GIT_INDEX_FILE, GIT_OBJECT_DIRECTORY and the temporary review dir.
    let diff=git::diff(&f.rt,&project,DiffArgs{
        workspace:"demo".into(),project:"demo".into(),
        paths:vec!["src/main.rs".into()],offset:0,limit:16384,task_id:None,stage:None
    }).await.unwrap();
    assert_eq!(diff["has_changes"],true);
    assert_eq!(diff["has_more"],false);
    let commit=git::commit(&f.rt,&project,CommitArgs{
        workspace:"demo".into(),project:"demo".into(),
        paths:vec!["src/main.rs".into()],
        message:"test: Windows Git subprocess path compatibility".into(),
        expected_head:diff["head"].as_str().unwrap().into(),
        expected_diff_sha256:diff["diff_sha256"].as_str().unwrap().into(),
        task_id:None,stage:None
    }).await.unwrap();
    let after=git::status(&f.rt,&project).await.unwrap();
    assert_eq!(after["head"],commit["commit"]);
    let history=git::log(&f.rt,&project,LogArgs{
        workspace:"demo".into(),project:"demo".into(),limit:1,task_id:None,stage:None
    }).await.unwrap();
    assert_eq!(history["commits"][0]["subject"],"test: Windows Git subprocess path compatibility");
}

#[cfg(windows)]
#[tokio::test]
async fn windows_git_review_normalizes_checked_out_crlf_without_filters(){
    let fixture=fixture(|_|{});
    let project=fixture.rt.project("demo","demo").unwrap();
    let git=|arguments:&[&str]|{
        let output=std::process::Command::new(&fixture.rt.config.git.executable)
            .current_dir(&project.root.path).args(arguments).output().unwrap();
        assert!(output.status.success(),
                "git {arguments:?}: {}",String::from_utf8_lossy(&output.stderr));
        output.stdout
    };
    git(&["init","--quiet","--initial-branch=dev"]);
    git(&["config","core.autocrlf","true"]);
    let source=project.root.path.join("src/main.rs");
    std::fs::write(&source,b"first\r\nsecond\r\n").unwrap();
    git(&["add","--","src/main.rs"]);
    git(&["-c","user.name=Test","-c","user.email=test@example.invalid",
          "commit","--quiet","-m","initial"]);
    let args=||DiffArgs{
        workspace:"demo".into(),project:"demo".into(),
        paths:vec!["src/main.rs".into()],offset:0,limit:16384,
        task_id:None,stage:None,
    };
    let clean=git::diff(&fixture.rt,&project,args()).await.unwrap();
    assert_eq!(clean["has_changes"],false,
              "CRLF checkout must not appear to modify an LF-indexed file");
    assert_eq!(clean["added_lines"],0);
    assert_eq!(clean["removed_lines"],0);

    std::fs::write(&source,b"first\r\nupdated\r\n").unwrap();
    let changed=git::diff(&fixture.rt,&project,args()).await.unwrap();
    assert_eq!(changed["has_changes"],true);
    assert_eq!(changed["added_lines"],1,
              "Only the edited line should appear in the review");
    assert_eq!(changed["removed_lines"],1);
    let commit=git::commit(&fixture.rt,&project,CommitArgs{
        workspace:"demo".into(),project:"demo".into(),
        paths:vec!["src/main.rs".into()],
        message:"test: Windows CRLF normalized snapshot".into(),
        expected_head:changed["head"].as_str().unwrap().into(),
        expected_diff_sha256:changed["diff_sha256"].as_str().unwrap().into(),
        task_id:None,stage:None,
    }).await.unwrap();
    assert_eq!(git::status(&fixture.rt,&project).await.unwrap()["head"],commit["commit"]);
    assert_eq!(git(&["show","HEAD:src/main.rs"]),b"first\nupdated\n");
    assert_eq!(std::fs::read(&source).unwrap(),b"first\r\nupdated\r\n");
}

#[test]
fn windows_deployment_keeps_cargo_build_output_separate_from_service(){
    let script=include_str!("../deploy/windows/EndlessVibe.ps1");
    for marker in ["target\\windows-stage", "Programs\\EndlessVibe",
                   "Get-FileHash", "Invoke-ServiceControl", "Start-Process",
                   "\"stop\"", "\"Update\"", "\"Start\"", "\"Status\""]{
        assert!(script.contains(marker),"Windows launcher missing {marker}");
    }
    for unsafe_action in ["Stop-Process", "taskkill /F", "Remove-Item -Recurse"]{
        assert!(!script.contains(unsafe_action),"Unsafe installer action {unsafe_action}");
    }
    assert!(include_str!("../README.md").contains("deploy\\windows\\EndlessVibe.ps1"));
    assert!(include_str!("../README.en.md").contains("deploy\\windows\\EndlessVibe.ps1"));
}

#[cfg(windows)]
#[test]
fn windows_install_manifest_can_publish_and_roll_back(){
    let script=std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("deploy/windows/Test-Manifest.ps1");
    let output=std::process::Command::new("powershell.exe")
        .args(["-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-File"])
        .arg(script).output().unwrap();
    assert!(output.status.success(),
        "PowerShell manifest transaction regression failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains(
        "Windows manifest atomic publish and rollback tests passed"));
    let installer=include_str!("../deploy/windows/EndlessVibe.ps1");
    assert!(installer.contains("Restore-EndlessVibeManifest"));
    assert!(installer.contains("Start-Installed -Exe $previous.Exe"));
    let publish=installer.find("$transaction = Publish-EndlessVibeManifest").unwrap();
    let stop=installer.find("Write-Host (Invoke-ServiceControl -Exe $controller -Verb \"stop\"").unwrap();
    assert!(publish<stop,
        "Manifest publication must succeed before gracefully stopping the service");
}

#[cfg(windows)]
#[test]
fn native_appcontainer_cli_probes_fixed_worker_without_service_start(){
    let temp=tempfile::tempdir().unwrap();
    let mut config=Config::default();
    config.security.data_dir=temp.path().join("state");
    util::private_dir(&config.security.data_dir).unwrap();
    let config_path=temp.path().join("config.toml");
    util::private_create(&config_path,toml::to_string(&config).unwrap().as_bytes()).unwrap();
    let binary=env!("CARGO_BIN_EXE_endlessvibe");

    let invalid=std::process::Command::new(binary)
        .arg("--internal-sandbox-worker").output().unwrap();
    assert!(!invalid.status.success(),
        "Host-user process must not invoke the restricted AppContainer worker");

    let output=std::process::Command::new(binary)
        .args(["--config",config_path.to_str().unwrap(),"--probe-windows-sandbox"])
        .output().unwrap();
    assert!(output.status.success(),"Native AppContainer CLI failed: {}",
        String::from_utf8_lossy(&output.stderr));
    let result:Value=serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"],"passed");
    assert_eq!(result["operation"],"hash_input");
    assert_eq!(result["appcontainer_token_verified"],true);
    assert_eq!(result["job_object_enforced"],true);
    let mapped=&result["loaded_modules"];
    assert_eq!(mapped["toolhelp_parent_verified"],true);
    assert_eq!(mapped["mapped_section_backing_paths_verified"],true);
    assert_eq!(mapped["pinned_file_ids_stable_across_snapshots"],true);
    assert!(mapped["pinned_file_count"].as_u64().unwrap()>1);
    assert_eq!(mapped["loaded_section_file_id_proven"],false);
    assert_eq!(mapped["snapshot_count"],2);
    assert_eq!(mapped["expected_worker_identity"],true);
    assert_eq!(mapped["outside_system32_module_rejected"],true);
    assert_eq!(mapped["future_or_transient_module_loads_proven"],false);
    assert_eq!(mapped["authenticode_signatures_verified"],false);
    assert!(mapped["pre_system32_count"].as_u64().unwrap()>0);
    assert!(mapped["post_system32_count"].as_u64().unwrap()>0);
    assert!(mapped["startup_snapshot_modules"].as_array().unwrap().iter()
        .any(|entry|entry=="fixed-worker.exe"));
    assert!(mapped["completed_snapshot_modules"].as_array().unwrap().iter()
        .any(|entry|entry=="fixed-worker.exe"));
    assert_eq!(result["network_capabilities_granted"],0);
    assert_eq!(result["arbitrary_commands_supported"],false);
    assert_eq!(result["general_execution_backend"],false);
    assert_eq!(result["profile_crash_recovery_journaled"],true);
    let signing=&result["signature_diagnostics"];
    assert_eq!(signing["offline"],true);
    assert_eq!(signing["catalog_signatures_checked"],false);
    assert_eq!(signing["trusted_publisher_pinned"],false);
    assert_eq!(signing["execution_gate"],false);
    assert!(signing["checks"].as_array().is_some());
    assert!(!signing.to_string().contains("\\Windows\\"),
        "Signature diagnostics must not expose host system paths");
    assert_eq!(result["logs"]["bounded"],true);
    assert_eq!(result["logs"]["capture"],"private_files_without_parent_handle_inheritance");
    assert_eq!(result["logs"]["limit_bytes"],65536);
    assert_eq!(result["logs"]["stdout_bytes"],b"hash_input: receipt generated\n".len());
    assert_eq!(result["logs"]["stdout_sha256"],util::digest(b"hash_input: receipt generated\n"));
    assert_eq!(result["logs"]["stderr_bytes"],0);
    assert_eq!(result["logs"]["stderr_sha256"],util::digest(b""));
    assert_eq!(result["trusted_worker"]["origin"],"current_endlessvibe_executable");
    assert_eq!(result["trusted_worker"]["verified_before_and_after"],true);
    assert_eq!(result["profile_cleanup_verified"],true);
    assert_eq!(result["trusted_worker"]["arbitrary_project_executables"],false);
    let imports=&result["trusted_worker"]["dependency_audit"];
    assert_eq!(imports["format"],"pe32_plus_amd64");
    assert_eq!(imports["static_import_allowlist"],true);
    assert_eq!(imports["system32_runtime_integrity_rechecked"],true);
    assert_eq!(imports["delay_loaded_modules"],0);
    assert_eq!(imports["unknown_modules_allowed"],false);
    assert_eq!(imports["relative_dll_paths_allowed"],false);
    assert!(imports["approved_import_modules"].as_array().is_some_and(|v|!v.is_empty()));
    assert!(imports["msvc_runtime_modules_verified_in_system32"].is_array());
    let trusted_bytes=std::fs::read(binary).unwrap();
    assert_eq!(result["trusted_worker"]["bytes"],trusted_bytes.len());
    assert_eq!(result["trusted_worker"]["sha256"],util::digest(&trusted_bytes));
    assert!(!result.to_string().contains("isolated fixed-operation hash probe"),
        "Sensitive staged input content must not appear in diagnostic output");

    let journal=endlessvibe::tools::windows_recovery::journal_dir(&config.security.data_dir);
    assert_eq!(std::fs::read_dir(journal).unwrap().count(),0,
        "Completed fixed AppContainer operations must clear recovery records");
    let guard=util::single_instance(&config.security.data_dir.join("service.lock")).unwrap();
    let busy=std::process::Command::new(binary)
        .args(["--config",config_path.to_str().unwrap(),"--probe-windows-sandbox"])
        .output().unwrap();
    assert!(!busy.status.success(),
        "Probe must not run while another process holds the service identity lock");
    drop(guard);
    assert!(!temp.path().join("state").join("service.pid").exists(),
        "Probing must not install a long-running Windows service instance");
}

#[tokio::test]async fn file_edits_require_current_hash_and_backup(){let f=fixture(|_|{});let w=f.rt.project("demo","demo").unwrap();let r=filesystem::read(&f.rt,&w,ReadArgs{workspace:"demo".into(),project:"demo".into(),path:"src/main.rs".into(),start_line:1,max_lines:2,task_id:None,stage:None}).unwrap();let hash=r["sha256"].as_str().unwrap().to_owned();let result=filesystem::patch(&f.rt,&w,PatchArgs{workspace:"demo".into(),project:"demo".into(),path:"src/main.rs".into(),expected_sha256:hash.clone(),edits:vec![Edit{old_text:"hello".into(),new_text:"world".into(),expected_occurrences:1}],task_id:None,stage:None}).unwrap();assert!(result["backup_id"].is_string());assert!(String::from_utf8(w.root.read("src/main.rs",4096).unwrap()).unwrap().contains("world"));let stale=filesystem::write(&f.rt,&w,WriteArgs{workspace:"demo".into(),project:"demo".into(),path:"src/main.rs".into(),content:"lost edit".into(),expected_sha256:hash,create_parents:false,task_id:None,stage:None});assert!(stale.is_err());}
#[tokio::test]async fn new_files_do_not_overwrite_existing_work(){let f=fixture(|_|{});let w=f.rt.project("demo","demo").unwrap();let a=WriteArgs{workspace:"demo".into(),project:"demo".into(),path:"docs/new.txt".into(),content:"new\n".into(),expected_sha256:"MISSING".into(),create_parents:true,task_id:None,stage:None};filesystem::write(&f.rt,&w,a.clone()).unwrap();assert!(filesystem::write(&f.rt,&w,a).is_err());}
#[tokio::test]async fn read_only_projects_refuse_changes(){let f=fixture(|c|{c.workspaces[0].projects[0].allow_write=false;c.workspaces[0].projects[0].allow_exec=false;c.workspaces[0].projects[0].allow_git_commit=false;});let w=f.rt.project("demo","demo").unwrap();assert!(filesystem::mkdir(&w,MakeDirectoryArgs{workspace:"demo".into(),project:"demo".into(),path:"new".into(),task_id:None,stage:None}).is_err());assert!(w.exec_allowed().is_err());}
#[tokio::test]async fn sibling_projects_have_independent_locks(){let f=fixture(|c|{let root=c.workspaces[0].path.clone();std::fs::create_dir_all(root.join("other")).unwrap();c.workspaces[0].projects.push(ProjectConfig{id:"other".into(),path:"other".into(),allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,allow_git_push:false,execution_profile:endlessvibe::config::default_project_profile(),environment:vec![]});});let a=f.rt.project("demo","demo").unwrap();let b=f.rt.project("demo","other").unwrap();let guard=a.lock.clone().try_lock_owned().unwrap();assert!(a.lock.clone().try_lock_owned().is_err());assert!(b.lock.clone().try_lock_owned().is_ok());drop(guard);}
#[tokio::test]async fn search_excludes_credentials_and_generated_files(){let f=fixture(|_|{});let w=f.rt.project("demo","demo").unwrap();std::fs::write(w.root.path.join(".env"),"hello SECRET=private").unwrap();std::fs::create_dir(w.root.path.join("target")).unwrap();std::fs::write(w.root.path.join("target/cache"),"hello generated").unwrap();let r=filesystem::search(&f.rt,&w,SearchArgs{workspace:"demo".into(),project:"demo".into(),query:"hello".into(),path:".".into(),regex:false,case_sensitive:true,max_results:100,task_id:None,stage:None}).unwrap();assert_eq!(r["matches"].as_array().unwrap().len(),1);assert!(!r.to_string().contains("SECRET"));}
#[tokio::test]
async fn projects_cannot_escape_to_server_state(){
    let f=fixture(|_|{});
    let w=f.rt.project("demo","demo").unwrap();
    #[cfg(unix)]
    let link_created={
        std::os::unix::fs::symlink(
            &f.rt.config.security.data_dir,w.root.path.join("escape")
        ).unwrap();
        true
    };
    #[cfg(windows)]
    let link_created=match std::os::windows::fs::symlink_dir(
        &f.rt.config.security.data_dir,w.root.path.join("escape")
    ){
        Ok(())=>true,
        Err(error) if error.raw_os_error()==Some(1314)=>{
            eprintln!("Windows symlink privilege is unavailable; link traversal assertion skipped");
            false
        },
        Err(error)=>panic!("Cannot create test directory symlink: {error}"),
    };
    if link_created{
        assert!(w.root.read("escape/owner.key",4096).is_err());
    }
    assert!(f.rt.project("demo","/etc").is_err());
}

#[tokio::test]async fn git_commit_preserves_unrelated_staging_and_working_files(){let f=fixture(|_|{});initialize_git(&f,true);let w=f.rt.project("demo","demo").unwrap();std::fs::write(w.root.path.join("other.txt"),"staged user work\n").unwrap();git_cli(&w.root.path,&["add","other.txt"]);let before=git_cli(&w.root.path,&["diff","--cached","--binary"]);std::fs::write(w.root.path.join("tracked.txt"),"reviewed\n").unwrap();std::fs::write(w.root.path.join("new.txt"),"new file\n").unwrap();let paths=vec!["tracked.txt".into(),"new.txt".into()];let d=git::diff(&f.rt,&w,DiffArgs{workspace:"demo".into(),project:"demo".into(),paths:paths.clone(),offset:0,limit:16384,task_id:None,stage:None}).await.unwrap();let result=git::commit(&f.rt,&w,CommitArgs{workspace:"demo".into(),project:"demo".into(),paths,message:"feat(test): reviewed snapshot".into(),expected_head:d["head"].as_str().unwrap().into(),expected_diff_sha256:d["diff_sha256"].as_str().unwrap().into(),task_id:None,stage:None}).await.unwrap();assert_eq!(result["pushed"],false);assert_eq!(git_cli(&w.root.path,&["show","HEAD:tracked.txt"]),b"reviewed\n");assert_eq!(git_cli(&w.root.path,&["show","HEAD:new.txt"]),b"new file\n");assert_eq!(git_cli(&w.root.path,&["show","HEAD:other.txt"]),b"base\n");assert_eq!(git_cli(&w.root.path,&["diff","--cached","--binary"]),before);assert_eq!(std::fs::read(w.root.path.join("other.txt")).unwrap(),b"staged user work\n");assert!(!w.root.path.join(".git/index.lock").exists());}
#[tokio::test]async fn git_rejects_stale_reviews(){let f=fixture(|_|{});initialize_git(&f,true);let w=f.rt.project("demo","demo").unwrap();std::fs::write(w.root.path.join("tracked.txt"),"first\n").unwrap();let paths=vec!["tracked.txt".into()];let d=git::diff(&f.rt,&w,DiffArgs{workspace:"demo".into(),project:"demo".into(),paths:paths.clone(),offset:0,limit:16384,task_id:None,stage:None}).await.unwrap();std::fs::write(w.root.path.join("tracked.txt"),"new user change\n").unwrap();let e=git::commit(&f.rt,&w,CommitArgs{workspace:"demo".into(),project:"demo".into(),paths,message:"fix(test): stale".into(),expected_head:d["head"].as_str().unwrap().into(),expected_diff_sha256:d["diff_sha256"].as_str().unwrap().into(),task_id:None,stage:None}).await;assert!(e.is_err());assert!(!w.root.path.join(".git/index.lock").exists());assert_eq!(std::fs::read(w.root.path.join("tracked.txt")).unwrap(),b"new user change\n");}
#[tokio::test]async fn git_refuses_selected_pre_staged_work(){let f=fixture(|_|{});initialize_git(&f,true);let w=f.rt.project("demo","demo").unwrap();std::fs::write(w.root.path.join("tracked.txt"),"user staged\n").unwrap();git_cli(&w.root.path,&["add","tracked.txt"]);let index=std::fs::read(w.root.path.join(".git/index")).unwrap();let paths=vec!["tracked.txt".into()];let d=git::diff(&f.rt,&w,DiffArgs{workspace:"demo".into(),project:"demo".into(),paths:paths.clone(),offset:0,limit:16384,task_id:None,stage:None}).await.unwrap();assert!(git::commit(&f.rt,&w,CommitArgs{workspace:"demo".into(),project:"demo".into(),paths,message:"fix(test): forbidden".into(),expected_head:d["head"].as_str().unwrap().into(),expected_diff_sha256:d["diff_sha256"].as_str().unwrap().into(),task_id:None,stage:None}).await.is_err());assert_eq!(std::fs::read(w.root.path.join(".git/index")).unwrap(),index);}
#[tokio::test]async fn git_can_make_first_commit(){let f=fixture(|_|{});initialize_git(&f,false);let w=f.rt.project("demo","demo").unwrap();let paths=vec!["src/main.rs".into()];let d=git::diff(&f.rt,&w,DiffArgs{workspace:"demo".into(),project:"demo".into(),paths:paths.clone(),offset:0,limit:16384,task_id:None,stage:None}).await.unwrap();assert_eq!(d["head"],"UNBORN");git::commit(&f.rt,&w,CommitArgs{workspace:"demo".into(),project:"demo".into(),paths,message:"feat: initial".into(),expected_head:"UNBORN".into(),expected_diff_sha256:d["diff_sha256"].as_str().unwrap().into(),task_id:None,stage:None}).await.unwrap();assert!(!git_cli(&w.root.path,&["rev-parse","HEAD"]).is_empty());}
#[tokio::test]async fn git_diff_is_paginated_with_stable_review_token(){let f=fixture(|_|{});initialize_git(&f,true);let w=f.rt.project("demo","demo").unwrap();std::fs::write(w.root.path.join("tracked.txt"),(0..300).map(|i|format!("changed-line-{i:03}\n")).collect::<String>()).unwrap();let paths=vec!["tracked.txt".into()];let first=git::diff(&f.rt,&w,DiffArgs{workspace:"demo".into(),project:"demo".into(),paths:paths.clone(),offset:0,limit:512,task_id:None,stage:None}).await.unwrap();assert_eq!(first["has_more"],true);assert!(first["total_bytes"].as_u64().unwrap()>first["diff"].as_str().unwrap().len() as u64);let second=git::diff(&f.rt,&w,DiffArgs{workspace:"demo".into(),project:"demo".into(),paths,offset:first["next_offset"].as_u64().unwrap(),limit:512,task_id:None,stage:None}).await.unwrap();assert_eq!(first["diff_sha256"],second["diff_sha256"]);assert_eq!(first["head"],second["head"]);assert!(second["offset"].as_u64().unwrap()>0);}

const NATIVE_JOB_MODE_FILE:&str="endlessvibe-native-job.fixture";
const NATIVE_JOB_READY:&str="ENDLESSVIBE_NATIVE_JOB_READY";
fn native_job_program()->String{
    std::env::current_exe().unwrap().file_stem().unwrap().to_str().unwrap().to_owned()
}
fn configure_native_job_fixture(config:&mut Config,allowed:bool){
    let executable=std::env::current_exe().unwrap();
    let mut paths=vec![executable.parent().unwrap().to_path_buf()];
    paths.extend(std::env::split_paths(&config.execution.path));
    config.execution.path=std::env::join_paths(paths).unwrap().into_string().unwrap();
    if allowed{config.execution.allowed_programs.push(native_job_program());}
}
fn native_job_command(f:&Fixture,mode:&str)->(String,Vec<String>){
    assert!(["trusted_host","environment","project_environment","wait","appcontainer"].contains(&mode));
    let project=f.rt.project("demo","demo").unwrap();
    std::fs::write(project.root.path.join(NATIVE_JOB_MODE_FILE),mode).unwrap();
    (native_job_program(),vec!["--exact".into(),"native_job_fixture_child".into(),
        "--nocapture".into(),"--test-threads=1".into()])
}
#[test]
fn native_job_fixture_child(){
    // Cargo's ordinary suite must never recurse or run a blocking child mode.
    // Only the exact subprocess invocation plus its temporary project marker
    // activates this helper; no extra environment keys are injected into jobs.
    let args=std::env::args().skip(1).collect::<Vec<_>>();
    if args!=["--exact","native_job_fixture_child","--nocapture","--test-threads=1"]{return;}
    let mode=std::fs::read_to_string(NATIVE_JOB_MODE_FILE).unwrap();
    assert!(std::env::var_os("ENDLESSVIBE_JOB_SUMMARY").is_some());
    match mode.as_str(){
        "trusted_host"=>println!("TRUSTED_HOST_OK"),
        "environment"=>{
            println!("{}",if std::env::var("TEST_DATABASE_URL").unwrap_or_default()
                =="postgres://user:password@127.0.0.1:19031/test"{"True"}else{"False"});
            println!("{}",if std::env::var("ENDLESSVIBE_JOB_SUMMARY").is_ok_and(|value|!value.is_empty())
                {"True"}else{"False"});
        },
        "project_environment"=>println!("{}",if std::env::var("PROJECT_SERVICE_URL").unwrap_or_default()
            =="http://127.0.0.1:19031"{"True"}else{"False"}),
        "appcontainer"=>{
            use std::io::Write;
            println!("{NATIVE_JOB_READY}");
            std::io::stdout().flush().unwrap();
            std::fs::write("native-lpac-result.txt",b"written by the isolated native child\n").unwrap();
            let summary=std::env::var_os("ENDLESSVIBE_JOB_SUMMARY").unwrap();
            std::fs::write(summary,br#"{"fixture":"native_appcontainer","project_write":true}"#).unwrap();
        },
        "wait"=>{
            use std::io::Write;
            println!("{NATIVE_JOB_READY}");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(30));
        },
        other=>panic!("Unknown native job fixture mode: {other}"),
    }
}
async fn wait_native_job_started(rt:&Arc<Runtime>,id:&str){
    for _ in 0..200{
        let output=rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:4096}).unwrap();
        if output["output"].as_str().unwrap().contains(NATIVE_JOB_READY){return;}
        let state=rt.jobs.get(id).unwrap();
        assert!(matches!(state["status"].as_str(),Some("queued"|"running")),
            "Native fixture stopped before readiness: {state}; {output}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("Native fixture did not report readiness: {}",rt.jobs.get(id).unwrap());
}
async fn wait_job(rt:&Arc<Runtime>,id:&str)->Value{for _ in 0..150{let j=rt.jobs.get(id).unwrap();if !matches!(j["status"].as_str(),Some("queued"|"running")){return j;}tokio::time::sleep(Duration::from_millis(30)).await;}panic!("Job did not finish");}

#[cfg(windows)]
fn configure_appcontainer_job_fixture(config:&mut Config){
    config.execution.backend="appcontainer".into();
    config.execution.acknowledge_unsafe_host_execution=false;
    config.execution.allow_shell=false;
    config.execution.allow_network=false;
    config.workspaces[0].projects[0].execution_profile="development".into();
}
#[cfg(windows)]
async fn wait_appcontainer_job(rt:&Arc<Runtime>,id:&str,seconds:u64)->Value{
    let until=tokio::time::Instant::now()+Duration::from_secs(seconds);
    loop{
        let state=rt.jobs.get(id).unwrap();
        if !matches!(state["status"].as_str(),Some("queued"|"running"))&&rt.jobs.active_count()==0{
            let project=rt.project(state["workspace"].as_str().unwrap(),state["project"].as_str().unwrap()).unwrap();
            if project.lock.clone().try_lock_owned().is_ok(){return state;}
        }
        if tokio::time::Instant::now()>=until{
            // Cancellation and shutdown are necessary even after an assertion
            // deadline; do not leave a native process or ACL lease behind.
            let _=rt.jobs.cancel(id);
            rt.jobs.shutdown().await;
            let output=rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:32768}).unwrap();
            panic!("AppContainer job exceeded the {seconds}s observation limit: {state}; {output}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
#[cfg(windows)]
fn assert_appcontainer_job_complete(f:&Fixture,job:&Value,output:&Value){
    assert_eq!(job["status"],"succeeded","{job}; {output}");
    assert_eq!(job["backend"],"appcontainer","{job}");
    assert_eq!(job["exit_code"],0,"{job}; {output}");
    let isolation=&job["preflight"]["isolation"];
    assert_eq!(isolation["kind"],"less_privileged_appcontainer","{job}");
    for key in ["token_verified","job_empty","cleanup_complete"]{
        assert_eq!(isolation[key],true,"Missing native isolation evidence {key}: {job}");
    }
    assert_eq!(isolation["capability_count"],1,"{job}");
    assert_eq!(isolation["capabilities"],json!(["registryRead"]),"{job}");
    assert_eq!(isolation["network"],false,"{job}");
    assert_eq!(job["summary_capture"]["status"],"captured","{job}; {output}");
    assert_eq!(job["summary_capture"]["reported"],true,"{job}");
    assert_eq!(job["summary_capture"]["cleanup_complete"],true,"{job}");
    assert_eq!(f.rt.jobs.active_count(),0);
    assert!(f.rt.project("demo","demo").unwrap().lock.clone().try_lock_owned().is_ok(),
        "Project lock must be released after native cleanup");
    for directory in ["appcontainer-acl","appcontainer-profiles","appcontainer-execution"]{
        let path=f.rt.config.security.data_dir.join(directory);
        assert_eq!(std::fs::read_dir(&path).unwrap().count(),0,
            "Completed native job retained a recovery record in {}",path.display());
    }
}
#[cfg(windows)]
#[tokio::test]
async fn windows_appcontainer_jobs_execute_native_tool_and_capture_summary(){
    let f=fixture(|config|{
        configure_appcontainer_job_fixture(config);
        config.execution.auto_discover_toolchains=false;
        // Cargo may hard-link test binaries across target directories. Copy
        // this exact executable into the fixture's independent tools bin;
        // granting target/deps would authorize unrelated build artifacts.
        let executable=std::env::current_exe().unwrap();
        let bin=config.security.data_dir.parent().unwrap().join("tools/native-job/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(&executable,bin.join(executable.file_name().unwrap())).unwrap();
        let mut paths=vec![bin];paths.extend(std::env::split_paths(&config.execution.path));
        config.execution.path=std::env::join_paths(paths).unwrap().into_string().unwrap();
        config.execution.allowed_programs=vec![native_job_program()];
        config.limits.command_timeout_seconds=20;
    });
    let (program,args)=native_job_command(&f,"appcontainer");
    let command=CommandArgs{workspace:"demo".into(),project:"demo".into(),program,args,
        cwd:".".into(),request_id:"native-lpac-job".into(),timeout_seconds:Some(20),
        preflight_programs:vec![],environment:Default::default(),network:None,task_id:None,stage:None};
    let job=f.rt.jobs.submit(f.rt.clone(),command,false).await.unwrap();
    assert_eq!(job["preflight"]["backend"],"appcontainer","{job}");
    assert_eq!(job["preflight"]["execution_profile"],"development","{job}");
    assert_eq!(job["preflight"]["network"],false,"{job}");
    assert_eq!(job["preflight"]["network_source"],"appcontainer_offline","{job}");
    assert!(f.rt.project("demo","demo").unwrap().lock.clone().try_lock_owned().is_err());
    let id=job["job_id"].as_str().unwrap();
    let finished=wait_appcontainer_job(&f.rt,id,30).await;
    let output=f.rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:32768}).unwrap();
    assert_appcontainer_job_complete(&f,&finished,&output);
    assert!(output["output"].as_str().unwrap().contains(NATIVE_JOB_READY),"{output}");
    assert_eq!(finished["summary"],json!({"fixture":"native_appcontainer","project_write":true}));
    assert_eq!(std::fs::read(f.rt.project("demo","demo").unwrap().root.path.join("native-lpac-result.txt")).unwrap(),
        b"written by the isolated native child\n");
}
#[cfg(windows)]
#[tokio::test]
async fn windows_appcontainer_jobs_compile_and_test_offline_cargo_project(){
    // Run this acceptance test separately from the short integration suite:
    // both its real compiler job and its observation deadline are 90 seconds.
    let f=fixture(|config|{
        configure_appcontainer_job_fixture(config);
        config.execution.auto_discover_toolchains=true;
        config.execution.allowed_programs=vec!["cargo".into(),"rustc".into()];
        config.limits.command_timeout_seconds=90;
    });
    let project=f.rt.project("demo","demo").unwrap();
    std::fs::write(project.root.path.join("Cargo.toml"),r#"[package]
name = "endlessvibe_native_lpac_fixture"
version = "0.0.0"
edition = "2021"

[workspace]

[profile.dev]
incremental = false
"#).unwrap();
    std::fs::write(project.root.path.join("Cargo.lock"),r#"version = 4

[[package]]
name = "endlessvibe_native_lpac_fixture"
version = "0.0.0"
"#).unwrap();
    std::fs::write(project.root.path.join("src/main.rs"),r##"fn add(left:u64,right:u64)->u64 { left+right }
fn main() { println!("{}",add(19,23)); }
#[test]
fn compiled_inside_native_appcontainer() {
    assert_eq!(add(19,23),42);
    let executable=std::env::current_exe().unwrap();
    println!("ENDLESSVIBE_OFFLINE_CARGO_COMPILED_TEST_OK {}",executable.display());
    std::fs::write("offline-cargo-result.txt",b"compiled and tested inside AppContainer\n").unwrap();
    let summary=std::env::var_os("ENDLESSVIBE_JOB_SUMMARY").unwrap();
    std::fs::write(summary,br#"{"fixture":"offline_cargo","compiled_test":true,"result":42}"#).unwrap();
}
"##).unwrap();
    let command=CommandArgs{workspace:"demo".into(),project:"demo".into(),program:"cargo".into(),
        args:["test","--offline","--locked","--target-dir","target/native-lpac","--","--nocapture",
            "--test-threads=1"].map(str::to_owned).to_vec(),cwd:".".into(),
        request_id:"native-lpac-offline-cargo".into(),timeout_seconds:Some(90),
        preflight_programs:vec!["rustc".into(),"cl".into(),"link".into(),"lib".into()],
        environment:Default::default(),network:None,task_id:None,stage:None};
    let job=f.rt.jobs.submit(f.rt.clone(),command,false).await.unwrap_or_else(|error|
        panic!("Real offline Cargo submission failed: {error:#}; configured PATH={}",f.rt.config.execution.path));
    eprintln!("Native offline Cargo resolved toolchains: {}",job["preflight"]);
    assert_eq!(job["preflight"]["backend"],"appcontainer","{job}");
    assert_eq!(job["preflight"]["network"],false,"{job}");
    assert_eq!(job["preflight"]["network_source"],"appcontainer_offline","{job}");
    assert_eq!(job["preflight"]["toolchains"]["rust"]["available"],true,"{job}");
    for program in ["cargo","rustc","cl","link","lib"]{
        let row=job["preflight"]["programs"].as_array().unwrap().iter()
            .find(|entry|entry["program"]==program).unwrap();
        assert!(Path::new(row["path"].as_str().unwrap()).is_file(),"{row}");
    }
    assert!(project.lock.clone().try_lock_owned().is_err());
    let id=job["job_id"].as_str().unwrap();
    let finished=wait_appcontainer_job(&f.rt,id,90).await;
    let output=f.rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:32768}).unwrap();
    assert_appcontainer_job_complete(&f,&finished,&output);
    let text=output["output"].as_str().unwrap();
    assert!(text.contains("ENDLESSVIBE_OFFLINE_CARGO_COMPILED_TEST_OK"),"{output}");
    assert!(text.contains("test result: ok. 1 passed; 0 failed"),"{output}");
    assert_eq!(finished["summary"],json!({"fixture":"offline_cargo","compiled_test":true,"result":42}));
    assert_eq!(std::fs::read(project.root.path.join("offline-cargo-result.txt")).unwrap(),
        b"compiled and tested inside AppContainer\n");
    let artifacts=project.root.path.join("target/native-lpac/debug/deps");
    assert!(std::fs::read_dir(&artifacts).unwrap().any(|entry|{
        let entry=entry.unwrap();let path=entry.path();
        path.file_stem().is_some_and(|stem|stem.to_string_lossy().starts_with("endlessvibe_native_lpac_fixture-"))
            &&path.extension().is_some_and(|extension|extension=="exe")
            &&entry.metadata().unwrap().len()>0
    }),"The sandbox must leave a real compiled test executable inside {}",artifacts.display());
}

#[tokio::test]
async fn trusted_host_project_can_run_unlisted_local_tool_without_shell_api(){
 let f=fixture(|c|{
  configure_native_job_fixture(c,false);
  c.execution.backend="host".into();
  c.execution.acknowledge_unsafe_host_execution=true;
  c.execution.allow_shell=false;
  c.workspaces[0].projects[0].execution_profile="trusted_host".into();
 });
 let (program,arguments)=native_job_command(&f,"trusted_host");
 assert!(!f.rt.config.execution.allowed_programs.contains(&program));
 let args=CommandArgs{workspace:"demo".into(),project:"demo".into(),
  program,args:arguments,
  cwd:".".into(),request_id:"trusted-host-shell-command".into(),
  timeout_seconds:Some(5),preflight_programs:vec![],
  environment:Default::default(),network:None,task_id:None,stage:None};
 let job=f.rt.jobs.submit(f.rt.clone(),args,false).await.unwrap();
 let id=job["job_id"].as_str().unwrap();
 let finished=wait_job(&f.rt,id).await;
 assert_eq!(finished["status"],"succeeded");
 let output=f.rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:1024}).unwrap();
 assert!(output["output"].as_str().unwrap().contains("TRUSTED_HOST_OK"));
 assert_eq!(job["preflight"]["execution_profile"],"trusted_host");
 let app=server::create_dashboard_router(f.rt.clone());
 let page=http(&app,"GET","/projects",Body::empty(),None,None,None).await;
 assert_eq!(page.status(),StatusCode::OK);
 let html=to_bytes(page.into_body(),2*1024*1024).await.unwrap();
 assert!(String::from_utf8_lossy(&html).contains("value=\"trusted_host\""));
 let script=http(&app,"GET","/assets/app.js",Body::empty(),None,None,None).await;
 let source=to_bytes(script.into_body(),2*1024*1024).await.unwrap();
 assert!(String::from_utf8_lossy(&source).contains("trusted_host"));
}
#[tokio::test]async fn jobs_return_output_and_deduplicate(){let f=fixture(|_|{});let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program:"git".into(),args:vec!["--version".into()],cwd:".".into(),request_id:"once".into(),timeout_seconds:Some(3),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let j=f.rt.jobs.submit(f.rt.clone(),a.clone(),false).await.unwrap();let id=j["job_id"].as_str().unwrap();let finished=wait_job(&f.rt,id).await;assert_eq!(finished["status"],"succeeded");let task_id=finished["task_id"].as_str().unwrap();assert!(task_id.starts_with("auto-job-"));assert_eq!(finished["stage"],"execute");let checkpoint=tasks::get(&f.rt.db,TaskArgs{workspace:"demo".into(),project:"demo".into(),task_id:task_id.into()}).unwrap();assert_eq!(checkpoint["latest"]["origin"],"auto_job");assert_eq!(checkpoint["latest"]["status"],"succeeded");let out=f.rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:4096}).unwrap();assert!(out["output"].as_str().unwrap().contains("git version"));let reused=f.rt.jobs.submit(f.rt.clone(),a.clone(),false).await.unwrap();assert_eq!(reused["job_id"],id);assert_eq!(reused["reused"],true);let mut changed=a;changed.args.push("different".into());assert!(f.rt.jobs.submit(f.rt.clone(),changed,false).await.is_err());}
#[tokio::test]async fn dashboard_lists_automatic_job_and_job_detail(){let f=fixture(|_|{});let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program:"git".into(),args:vec!["--version".into()],cwd:".".into(),request_id:"dashboard-auto-job".into(),timeout_seconds:Some(3),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let j=f.rt.jobs.submit(f.rt.clone(),a,false).await.unwrap();let id=j["job_id"].as_str().unwrap();assert_eq!(wait_job(&f.rt,id).await["status"],"succeeded");let dashboard=server::create_dashboard_router(f.rt.clone());let data=json_body(http(&dashboard,"GET","/api/tasks",Body::empty(),None,None,None).await).await;let checkpoints=data["checkpoints"].as_array().unwrap();assert_eq!(checkpoints.len(),1);assert_eq!(checkpoints[0]["origin"],"auto_job");assert_eq!(checkpoints[0]["status"],"succeeded");assert!(checkpoints[0]["last_commit"].is_null());assert_eq!(checkpoints[0]["program"],"git");assert_eq!(checkpoints[0]["request_id"],"dashboard-auto-job");assert_eq!(data["total_auto_jobs"],1);assert_eq!(data["total_explicit_tasks"],0);let detail=http(&dashboard,"GET",&format!("/api/jobs/{id}"),Body::empty(),None,None,None).await;assert_eq!(detail.status(),StatusCode::OK);let payload=json_body(detail).await;assert!(payload["output"]["output"].as_str().unwrap().contains("git version"));}
#[tokio::test]async fn task_tool_links_file_operations_and_dashboard_shows_them(){let f=fixture(|_|{});let app=server::create_router(f.rt.clone());let token=f.rt.auth.issue_local_token().unwrap();let start=json!({"jsonrpc":"2.0","id":51,"method":"tools/call","params":{"name":"start_task","arguments":{"workspace":"demo","project":"demo","task_id":"feature-fft","stage":"inspect"}}});let response=http(&app,"POST","/mcp",Body::from(start.to_string()),Some("application/json"),Some(&token),None).await;assert_eq!(response.status(),StatusCode::OK);let read=json!({"jsonrpc":"2.0","id":52,"method":"tools/call","params":{"name":"read_file","arguments":{"workspace":"demo","project":"demo","path":"src/main.rs","task_id":"feature-fft","stage":"inspect"}}});let response=http(&app,"POST","/mcp",Body::from(read.to_string()),Some("application/json"),Some(&token),None).await;assert_eq!(response.status(),StatusCode::OK);let cp=tasks::get(&f.rt.db,TaskArgs{workspace:"demo".into(),project:"demo".into(),task_id:"feature-fft".into()}).unwrap();let ops=cp["latest"]["operations"].as_array().unwrap();assert!(ops.iter().any(|op|op["tool"]=="read_file"&&op["status"]=="succeeded"));assert_eq!(cp["latest"]["status"],"pending");let dashboard=server::create_dashboard_router(f.rt.clone());let data=json_body(http(&dashboard,"GET","/api/tasks",Body::empty(),None,None,None).await).await;assert!(data["checkpoints"].as_array().unwrap().iter().any(|item|item["task_id"]=="feature-fft"));assert_eq!(data["total_explicit_tasks"],1);assert_eq!(data["total_auto_jobs"],0);}
#[tokio::test]async fn retained_legacy_job_is_backfilled_once(){let f=fixture(|_|{});let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program:"git".into(),args:vec!["--version".into()],cwd:".".into(),request_id:"legacy-for-backfill".into(),timeout_seconds:Some(3),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let started=f.rt.jobs.submit(f.rt.clone(),a,false).await.unwrap();let id=started["job_id"].as_str().unwrap();let finished=wait_job(&f.rt,id).await;let task=finished["task_id"].as_str().unwrap().to_owned();f.rt.db.transaction(|tx|{tx.execute("UPDATE jobs SET data=json_set(data,'$.task_id',NULL,'$.stage',NULL) WHERE id=?1",[id])?;tx.execute("DELETE FROM kv WHERE namespace='task_checkpoints'",[])?;Ok(())}).unwrap();let restored=endlessvibe::tools::jobs::Jobs::new(f.rt.db.clone(),f.rt.config.clone()).unwrap();let restored_job=restored.get(id).unwrap();assert_eq!(restored_job["task_id"],task);let checkpoint=tasks::get(&f.rt.db,TaskArgs{workspace:"demo".into(),project:"demo".into(),task_id:task}).unwrap();assert_eq!(checkpoint["latest"]["jobs"].as_array().unwrap().len(),1);assert_eq!(checkpoint["latest"]["status"],"succeeded");}
#[tokio::test]async fn job_environment_is_injected_without_exposing_values(){let f=fixture(|c|configure_native_job_fixture(c,true));let(program,args)=native_job_command(&f,"environment");let mut environment=std::collections::BTreeMap::new();environment.insert("TEST_DATABASE_URL".into(),"postgres://user:password@127.0.0.1:19031/test".into());let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program,args,cwd:".".into(),request_id:"job-env".into(),timeout_seconds:Some(3),preflight_programs:vec![],environment,network:Some(false),task_id:None,stage:None};let j=f.rt.jobs.submit(f.rt.clone(),a,false).await.unwrap();assert_eq!(j["preflight"]["environment_keys"],json!(["TEST_DATABASE_URL"]));assert!(!j.to_string().contains("password@"));let id=j["job_id"].as_str().unwrap();assert_eq!(wait_job(&f.rt,id).await["status"],"succeeded");let out=f.rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:4096}).unwrap();assert_eq!(out["output"].as_str().unwrap().matches("True").count(),2);}
#[tokio::test]async fn development_profile_supplies_project_execution_defaults(){let f=fixture(|c|{configure_native_job_fixture(c,true);let project=&mut c.workspaces[0].projects[0];project.execution_profile="development".into();project.environment=vec!["PROJECT_SERVICE_URL=http://127.0.0.1:19031".into()];});let(program,args)=native_job_command(&f,"project_environment");let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program,args,cwd:".".into(),request_id:"profile-defaults".into(),timeout_seconds:Some(3),preflight_programs:vec![],environment:Default::default(),network:None,task_id:None,stage:None};let j=f.rt.jobs.submit(f.rt.clone(),a,false).await.unwrap();assert_eq!(j["preflight"]["execution_profile"],"development");assert_eq!(j["preflight"]["network"],true);assert_eq!(j["preflight"]["network_source"],"development_profile");assert_eq!(j["preflight"]["environment_keys"],json!(["PROJECT_SERVICE_URL"]));let summary=f.rt.projects_for("demo").unwrap().to_string();assert!(summary.contains("PROJECT_SERVICE_URL"));assert!(!summary.contains("http://127.0.0.1:19031"));let id=j["job_id"].as_str().unwrap();assert_eq!(wait_job(&f.rt,id).await["status"],"succeeded");let out=f.rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:4096}).unwrap();assert!(out["output"].as_str().unwrap().contains("True"));}
#[tokio::test]async fn cached_legacy_project_addressing_normalizes_job_identity(){let f=fixture(|c|c.workspaces[0].id="root".into());let legacy=CommandArgs{workspace:"demo".into(),project:"".into(),program:"git".into(),args:vec!["--version".into()],cwd:".".into(),request_id:"legacy-once".into(),timeout_seconds:Some(3),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let j=f.rt.jobs.submit(f.rt.clone(),legacy.clone(),false).await.unwrap();let id=j["job_id"].as_str().unwrap().to_owned();let finished=wait_job(&f.rt,&id).await;assert_eq!(finished["workspace"],"root");assert_eq!(finished["project"],"demo");assert_eq!(j["legacy_addressing"],true);let mut canonical=legacy;canonical.workspace="root".into();canonical.project="demo".into();let reused=f.rt.jobs.submit(f.rt.clone(),canonical,false).await.unwrap();assert_eq!(reused["job_id"],id);assert_eq!(reused["reused"],true);let app=server::create_router(f.rt.clone());let token=f.rt.auth.issue_local_token().unwrap();let call=json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_jobs","arguments":{"workspace":"demo","limit":20}}});let response=http(&app,"POST","/mcp",Body::from(call.to_string()),Some("application/json"),Some(&token),None).await;assert_eq!(response.status(),StatusCode::OK);let data=json_body(response).await;assert!(data.to_string().contains(&id));}
#[tokio::test]async fn job_cancellation_releases_project_lock(){let f=fixture(|c|configure_native_job_fixture(c,true));let(program,args)=native_job_command(&f,"wait");let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program,args,cwd:".".into(),request_id:"cancel-me".into(),timeout_seconds:Some(60),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let j=f.rt.jobs.submit(f.rt.clone(),a,false).await.unwrap();let id=j["job_id"].as_str().unwrap();wait_native_job_started(&f.rt,id).await;assert!(f.rt.project("demo","demo").unwrap().lock.clone().try_lock_owned().is_err());f.rt.jobs.cancel(id).unwrap();let finished=wait_job(&f.rt,id).await;assert_eq!(finished["status"],"cancelled");for _ in 0..20{if f.rt.jobs.active_count()==0{break;}tokio::time::sleep(Duration::from_millis(10)).await;}assert!(f.rt.project("demo","demo").unwrap().lock.clone().try_lock_owned().is_ok());}
#[tokio::test]async fn job_timeout_is_a_terminal_state(){let f=fixture(|c|{configure_native_job_fixture(c,true);c.limits.command_timeout_seconds=1;});let(program,args)=native_job_command(&f,"wait");let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program,args,cwd:".".into(),request_id:"timeout".into(),timeout_seconds:Some(1),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let j=f.rt.jobs.submit(f.rt.clone(),a,false).await.unwrap();let id=j["job_id"].as_str().unwrap();assert_eq!(wait_job(&f.rt,id).await["status"],"timed_out");let output=f.rt.jobs.output(OutputArgs{job_id:id.into(),offset:0,limit:4096}).unwrap();assert!(output["output"].as_str().unwrap().contains(NATIVE_JOB_READY),"Timeout must interrupt a real started process: {output}");}

async fn http(app:&Router,method:&str,uri:&str,body:Body,content_type:Option<&str>,token:Option<&str>,cookie:Option<&str>)->axum::response::Response{let mut r=Request::builder().method(method).uri(uri).header(header::HOST,"localhost");if let Some(t)=content_type{r=r.header(header::CONTENT_TYPE,t);}if let Some(t)=token{r=r.header(header::AUTHORIZATION,format!("Bearer {t}"));}if let Some(c)=cookie{r=r.header(header::COOKIE,c);}r=r.header(header::ACCEPT,"application/json, text/event-stream").header("MCP-Protocol-Version","2025-06-18");app.clone().oneshot(r.body(body).unwrap()).await.unwrap()}
async fn json_body(r:axum::response::Response)->Value{serde_json::from_slice(&to_bytes(r.into_body(),2*1024*1024).await.unwrap()).unwrap()}
fn form(fields:&[(&str,&str)])->String{url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields.iter().copied()).finish()}
async fn consent_http(app:&Router,body:String,cookie:Option<&str>,origin:&str)->axum::response::Response{
    let mut request=Request::builder().method("POST").uri("/oauth/authorize").header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/x-www-form-urlencoded").header(header::ORIGIN,origin);
    if let Some(cookie)=cookie{request=request.header(header::COOKIE,cookie);}
    app.clone().oneshot(request.body(Body::from(body)).unwrap()).await.unwrap()
}
async fn oauth_login(f:&Fixture,app:&Router,scope:&str)->(String,Value){
    let response=http(app,"POST","/oauth/register",Body::from(json!({"client_name":"Test","redirect_uris":["https://chatgpt.com/connector_platform_oauth_redirect"],"token_endpoint_auth_method":"none"}).to_string()),Some("application/json"),None,None).await;assert_eq!(response.status(),StatusCode::CREATED);let client=json_body(response).await["client_id"].as_str().unwrap().to_owned();
    let verifier="v".repeat(43);let challenge=URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));let resource=f.rt.config.resource();let authorize=format!("/oauth/authorize?{}",form(&[("client_id",&client),("redirect_uri","https://chatgpt.com/connector_platform_oauth_redirect"),("response_type","code"),("state","test-state"),("code_challenge",&challenge),("code_challenge_method","S256"),("resource",&resource),("scope",scope)]));
    let response=http(app,"GET",&authorize,Body::empty(),None,None,None).await;assert_eq!(response.status(),StatusCode::OK);assert_eq!(response.headers()[header::REFERRER_POLICY],"same-origin");let cookie=response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned();let html=String::from_utf8(to_bytes(response.into_body(),32768).await.unwrap().to_vec()).unwrap();let transaction=html.split("name=\"transaction\" value=\"").nth(1).unwrap().split('"').next().unwrap();
    let origin=f.rt.config.public_url().unwrap().origin().ascii_serialization();let response=consent_http(app,form(&[("transaction",transaction),("owner_key",&f.owner),("decision","approve")]),Some(&cookie),&origin).await;assert_eq!(response.status(),StatusCode::SEE_OTHER);let redirect=url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();let code=redirect.query_pairs().find(|(k,_)|k=="code").unwrap().1.to_string();let iss=redirect.query_pairs().find(|(k,_)|k=="iss").unwrap().1.to_string();assert_eq!(iss,f.rt.config.server.public_url);
    let request=form(&[("grant_type","authorization_code"),("client_id",&client),("redirect_uri","https://chatgpt.com/connector_platform_oauth_redirect"),("code",&code),("code_verifier",&verifier),("resource",&resource)]);let response=http(app,"POST","/oauth/token",Body::from(request.clone()),Some("application/x-www-form-urlencoded"),None,None).await;assert_eq!(response.status(),StatusCode::OK);let tokens=json_body(response).await;assert_eq!(http(app,"POST","/oauth/token",Body::from(request),Some("application/x-www-form-urlencoded"),None,None).await.status(),StatusCode::BAD_REQUEST);(client,tokens)
}
#[tokio::test]
async fn lan_only_exposes_oauth_locally_and_requires_pairing_for_lan_nodes(){
    let f=fixture(|config|config.enable_lan_only());
    assert_eq!(f.rt.config.server.bind.to_string(),"127.0.0.1:20000");
    assert!(f.rt.config.server.lan_only && f.rt.transfer.config.enabled);
    let mcp=server::create_router(f.rt.clone());
    let metadata=json_body(http(&mcp,"GET","/.well-known/oauth-protected-resource",Body::empty(),None,None,None).await).await;
    assert_eq!(metadata["resource"],"http://127.0.0.1:20000/mcp");
    assert_eq!(metadata["authorization_servers"][0],"http://127.0.0.1:20000");
    assert_eq!(http(&mcp,"POST","/mcp",Body::from("{}"),Some("application/json"),None,None).await.status(),StatusCode::UNAUTHORIZED);
    let unauthorized_host=Request::builder().uri("/health").header(header::HOST,"192.168.1.10").body(Body::empty()).unwrap();
    assert_eq!(mcp.clone().oneshot(unauthorized_host).await.unwrap().status(),StatusCode::FORBIDDEN);
    let local=server::create_dashboard_router(f.rt.clone());
    assert_eq!(http(&local,"GET","/nodes",Body::empty(),None,None,None).await.status(),StatusCode::OK);
    let lan_dashboard=Request::builder().uri("/nodes").header(header::HOST,"192.168.1.10").body(Body::empty()).unwrap();
    assert_eq!(local.oneshot(lan_dashboard).await.unwrap().status(),StatusCode::FORBIDDEN);
    assert!(f.rt.transfer.peers().unwrap()["peers"].as_array().unwrap().is_empty());
}
#[tokio::test]async fn dashboard_renders_docker_config_and_tool_group(){let f=fixture(|_|{});let app=server::create_dashboard_router(f.rt.clone());for (path,expected) in [("/config","id=\"docker-editor\""),("/assets/app.js","handleDockerSave"),("/assets/js/mcp.js","docker_restart")]{let response=http(&app,"GET",path,Body::empty(),None,None,None).await;assert_eq!(response.status(),StatusCode::OK);let bytes=to_bytes(response.into_body(),2*1024*1024).await.unwrap();let html=String::from_utf8(bytes.to_vec()).unwrap();assert!(html.contains(expected),"Missing {expected} at {path}");}}
#[tokio::test]async fn public_health_does_not_expose_paths_or_secrets(){let f=fixture(|_|{});let app=server::create_router(f.rt.clone());let response=http(&app,"GET","/health",Body::empty(),None,None,None).await;assert_eq!(response.status(),StatusCode::OK);let data=json_body(response).await.to_string();assert!(!data.contains(&f.owner));assert!(!data.contains(&f.rt.config.security.data_dir.to_string_lossy().to_string()));assert_eq!(http(&app,"POST","/mcp",Body::from("{}"),Some("application/json"),None,None).await.status(),StatusCode::UNAUTHORIZED);}
#[tokio::test]async fn cache_maintenance_lock_blocks_new_jobs_without_execution(){let f=fixture(|_|{});let guard=f.rt.jobs.begin_cache_cleanup().unwrap();let a=CommandArgs{workspace:"demo".into(),project:"demo".into(),program:"git".into(),args:vec!["--version".into()],cwd:".".into(),request_id:"cache-race-guard".into(),timeout_seconds:Some(3),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let error=f.rt.jobs.submit(f.rt.clone(),a.clone(),false).await.unwrap_err();assert_eq!(endlessvibe::error::code(&error),"CACHE_BUSY");assert_eq!(f.rt.jobs.active_count(),0);drop(guard);let job=f.rt.jobs.submit(f.rt.clone(),a,false).await.unwrap();assert_eq!(wait_job(&f.rt,job["job_id"].as_str().unwrap()).await["status"],"succeeded");}
#[tokio::test]async fn dashboard_cache_cleanup_checks_confirmation_and_active_jobs(){let f=fixture(|c|configure_native_job_fixture(c,true));let(program,args)=native_job_command(&f,"wait");let cache=f.rt.config.security.data_dir.join("exec-cache");let external=f._dir.path().join("outside.txt");std::fs::write(&external,b"preserve").unwrap();std::fs::create_dir_all(cache.join("demo/demo/build")).unwrap();std::fs::write(cache.join("demo/demo/build/out.bin"),vec![0u8;8192]).unwrap();let app=server::create_dashboard_router(f.rt.clone());let post=|body:Value|Request::builder().method("POST").uri("/api/storage/cleanup").header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json").header(header::ORIGIN,"http://localhost:20001").body(Body::from(body.to_string())).unwrap();let wrong=app.clone().oneshot(post(json!({"scope":"exec_cache","confirmation":"no"}))).await.unwrap();assert_eq!(wrong.status(),StatusCode::BAD_REQUEST);assert!(cache.join("demo/demo/build/out.bin").exists());let command=CommandArgs{workspace:"demo".into(),project:"demo".into(),program,args,cwd:".".into(),request_id:"guard-cache".into(),timeout_seconds:Some(60),preflight_programs:vec![],environment:Default::default(),network:Some(false),task_id:None,stage:None};let job=f.rt.jobs.submit(f.rt.clone(),command,false).await.unwrap();let job_id=job["job_id"].as_str().unwrap();wait_native_job_started(&f.rt,job_id).await;let busy=app.clone().oneshot(post(json!({"scope":"exec_cache","confirmation":"CLEAR_EXEC_CACHE"}))).await.unwrap();assert_eq!(busy.status(),StatusCode::CONFLICT);f.rt.jobs.cancel(job_id).unwrap();assert_eq!(wait_job(&f.rt,job_id).await["status"],"cancelled");for _ in 0..100{if f.rt.jobs.active_count()==0{break;}tokio::time::sleep(Duration::from_millis(10)).await;}assert_eq!(f.rt.jobs.active_count(),0);let response=app.clone().oneshot(post(json!({"scope":"exec_cache","confirmation":"CLEAR_EXEC_CACHE"}))).await.unwrap();assert_eq!(response.status(),StatusCode::OK);let result=json_body(response).await;assert_eq!(result["cleared"],true);assert!(result["estimated_reclaim_bytes_at_least"].as_u64().unwrap()>=8192);assert!(!cache.join("demo/demo/build/out.bin").exists());assert_eq!(std::fs::read(&external).unwrap(),b"preserve");let compact=Request::builder().method("POST").uri("/api/storage/compact").header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json").header(header::ORIGIN,"http://localhost:20001").body(Body::from("{}")).unwrap();let compact=app.clone().oneshot(compact).await.unwrap();assert_eq!(compact.status(),StatusCode::OK);let result=json_body(compact).await;assert_eq!(result["compaction"]["compacted"],true);}
#[cfg(unix)]
#[tokio::test]async fn dashboard_docker_edit_is_explicit_and_needs_restart(){let f=fixture(|_|{});let app=server::create_dashboard_router(f.rt.clone());let before=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;assert_eq!(before["docker"]["enabled"],false);let request=json!({"expected_revision":before["revision"],"enabled":true,"socket":"/var/run/docker.sock","allowed_containers":["endlessvibe"],"allow_start":false,"allow_stop":false,"allow_restart":true,"allow_project_socket":false,"acknowledge_daemon_control":false});let put=|v:Value|Request::builder().method("PUT").uri("/api/config/docker").header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json").header(header::ORIGIN,"http://localhost:20001").body(Body::from(v.to_string())).unwrap();assert_eq!(app.clone().oneshot(put(request.clone())).await.unwrap().status(),StatusCode::BAD_REQUEST);let mut allowed=request.clone();allowed["acknowledge_daemon_control"]=json!(true);let response=app.clone().oneshot(put(allowed.clone())).await.unwrap();assert_eq!(response.status(),StatusCode::OK);let result=json_body(response).await;assert_eq!(result["docker"]["allow_restart"],true);assert_eq!(result["requires_restart"],true);assert_eq!(app.clone().oneshot(put(allowed)).await.unwrap().status(),StatusCode::CONFLICT);let status=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;assert_eq!(status["docker"]["allowed_containers"],json!(["endlessvibe"]));assert_eq!(status["requires_restart"],true);}
#[tokio::test]async fn docker_logs_are_returned_but_not_retained_in_operation_db(){let f=fixture(|_|{});let trace=f.rt.begin_operation("docker_logs","","",json!({"container":"allowed"}));let id=trace.id.unwrap();let v=f.rt.finish_operation(trace,Ok(json!({"container":"allowed","logs":"SENSITIVE_LOG_TOKEN_123"}))).unwrap();assert_eq!(v["logs"],"SENSITIVE_LOG_TOKEN_123");let persisted=f.rt.db.operation(id).unwrap();assert_eq!(persisted["output"]["logs"]["omitted"],true);assert!(!persisted.to_string().contains("SENSITIVE_LOG_TOKEN_123"));}
#[tokio::test]async fn docker_read_scope_does_not_grant_container_actions(){let f=fixture(|_|{});let app=server::create_router(f.rt.clone());let(_,tokens)=oauth_login(&f,&app,"docker:read").await;let bearer=tokens["access_token"].as_str().unwrap();let call=json!({"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"docker_restart","arguments":{"container":"endlessvibe","confirm":true}}});let response=http(&app,"POST","/mcp",Body::from(call.to_string()),Some("application/json"),Some(bearer),None).await;assert_eq!(response.status(),StatusCode::OK);let v=json_body(response).await;assert_eq!(v["result"]["isError"],true);assert!(v.to_string().contains("insufficient_scope"));assert!(v.to_string().contains("docker:write"));let read=json!({"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"docker_list","arguments":{"limit":10}}});let response=http(&app,"POST","/mcp",Body::from(read.to_string()),Some("application/json"),Some(bearer),None).await;assert_eq!(response.status(),StatusCode::OK);let v=json_body(response).await;assert!(v.to_string().contains("Docker MCP is disabled"));}
#[tokio::test]async fn dashboard_edits_git_and_limits_with_revision_conflicts(){let f=fixture(|_|{});let app=server::create_dashboard_router(f.rt.clone());let before=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;let first=json!({"expected_revision":before["revision"],"executable":before["git"]["executable"],"author_name":"Web Author","author_email":"web@example.org"});let put=|url:&str,body:serde_json::Value|Request::builder().method("PUT").uri(url).header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json").header(header::ORIGIN,"http://localhost:20001").body(Body::from(body.to_string())).unwrap();let response=app.clone().oneshot(put("/api/config/git",first.clone())).await.unwrap();assert_eq!(response.status(),StatusCode::OK);let saved=json_body(response).await;assert_eq!(saved["requires_restart"],true);let stale=app.clone().oneshot(put("/api/config/git",first)).await.unwrap();assert_eq!(stale.status(),StatusCode::CONFLICT);let limits=before["limits"].as_object().unwrap();let mut patch=limits.clone();patch.insert("expected_revision".into(),saved["revision"].clone());patch.insert("command_timeout_seconds".into(),json!(600));patch.insert("retained_jobs".into(),json!(20));let response=app.clone().oneshot(put("/api/config/limits",serde_json::Value::Object(patch))).await.unwrap();assert_eq!(response.status(),StatusCode::OK);let changed=json_body(response).await;assert_eq!(changed["limits"]["command_timeout_seconds"],600);assert_eq!(changed["requires_restart"],true);let after=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;assert_eq!(after["git"]["author_name"],"Web Author");assert_eq!(after["limits"]["retained_jobs"],20);assert_eq!(after["requires_restart"],true);}
#[tokio::test]async fn dashboard_can_edit_readonly_mounts_and_marks_restart_required(){let f=fixture(|_|{});let source=f._dir.path().join("android-sdk");std::fs::create_dir_all(&source).unwrap();let app=server::create_dashboard_router(f.rt.clone());let before=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;let revision=before["revision"].as_str().unwrap();let body=json!({"expected_revision":revision,"readonly_mounts":[{"source":source,"target":"/opt/android-sdk"}]});let request=Request::builder().method("PUT").uri("/api/config/execution/readonly-mounts").header(header::HOST,"localhost").header(header::CONTENT_TYPE,"application/json").header(header::ORIGIN,"http://localhost:20001").body(Body::from(body.to_string())).unwrap();let response=app.clone().oneshot(request).await.unwrap();assert_eq!(response.status(),StatusCode::OK);let saved=json_body(response).await;assert_eq!(saved["requires_restart"],true);assert_eq!(saved["execution"]["readonly_mounts"][0]["target"],"/opt/android-sdk");let after=json_body(http(&app,"GET","/api/config",Body::empty(),None,None,None).await).await;assert_eq!(after["execution"]["readonly_mounts"][0]["target"],"/opt/android-sdk");assert_eq!(after["requires_restart"],true);}
#[tokio::test]async fn oauth_authorization_and_scope_enforcement(){let f=fixture(|_|{});let app=server::create_router(f.rt.clone());let(_,tokens)=oauth_login(&f,&app,"files:read").await;let bearer=tokens["access_token"].as_str().unwrap();let call=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"run_command","arguments":{"workspace":"demo","project":"demo","program":"git","args":["--version"],"request_id":"blocked"}}});let response=http(&app,"POST","/mcp",Body::from(call.to_string()),Some("application/json"),Some(bearer),None).await;assert_eq!(response.status(),StatusCode::OK);let result=json_body(response).await;assert_eq!(result["result"]["isError"],true);let challenge=result["result"]["_meta"]["mcp/www_authenticate"][0].as_str().unwrap();assert!(challenge.contains("error=\"insufficient_scope\""));assert!(challenge.contains("commands:execute files:write"));assert_eq!(f.rt.jobs.active_count(),0);let read=json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_file","arguments":{"workspace":"demo","project":"demo","path":"src/main.rs"}}});let response=http(&app,"POST","/mcp",Body::from(read.to_string()),Some("application/json"),Some(bearer),None).await;assert_eq!(response.status(),StatusCode::OK);let result=json_body(response).await;assert_ne!(result["result"]["isError"],true);assert!(result["result"]["structuredContent"].is_object());}
#[tokio::test]async fn refresh_rotation_replay_revokes_the_family(){let f=fixture(|_|{});let app=server::create_router(f.rt.clone());let(client,tokens)=oauth_login(&f,&app,"files:read").await;let body=form(&[("grant_type","refresh_token"),("client_id",&client),("refresh_token",tokens["refresh_token"].as_str().unwrap()),("resource",&f.rt.config.resource())]);let response=http(&app,"POST","/oauth/token",Body::from(body.clone()),Some("application/x-www-form-urlencoded"),None,None).await;assert_eq!(response.status(),StatusCode::OK);let rotated=json_body(response).await;assert_eq!(http(&app,"POST","/oauth/token",Body::from(body),Some("application/x-www-form-urlencoded"),None,None).await.status(),StatusCode::BAD_REQUEST);assert_eq!(http(&app,"POST","/mcp",Body::from("{}"),Some("application/json"),rotated["access_token"].as_str(),None).await.status(),StatusCode::UNAUTHORIZED);}
#[tokio::test]async fn mcp_initialization_and_tools_are_real(){let f=fixture(|_|{});let app=server::create_router(f.rt.clone());let token=f.rt.auth.issue_local_token().unwrap();let init=json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"tests","version":"1"}}});let response=http(&app,"POST","/mcp",Body::from(init.to_string()),Some("application/json"),Some(&token),None).await;assert_eq!(response.status(),StatusCode::OK);let data=json_body(response).await;assert_eq!(data["result"]["serverInfo"]["name"],"EndlessVibe");let response=http(&app,"POST","/mcp",Body::from(json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string()),Some("application/json"),Some(&token),None).await;assert_eq!(response.status(),StatusCode::ACCEPTED);let response=http(&app,"POST","/mcp",Body::from(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}).to_string()),Some("application/json"),Some(&token),None).await;assert_eq!(response.status(),StatusCode::OK);let list=json_body(response).await;let tools=list["result"]["tools"].as_array().unwrap();assert_eq!(tools.len(),mcp::TOOL_NAMES.len());for name in mcp::TOOL_NAMES{assert!(tools.iter().any(|t|t["name"]==*name),"missing {name}");}let response=http(&app,"POST","/mcp",Body::from(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_workspaces","arguments":{}}}).to_string()),Some("application/json"),Some(&token),None).await;let data=json_body(response).await;assert_ne!(data["result"]["isError"],true);assert!(data.to_string().contains("demo"));let response=http(&app,"POST","/mcp",Body::from(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_projects","arguments":{"workspace":"demo"}}}).to_string()),Some("application/json"),Some(&token),None).await;let data=json_body(response).await;assert_ne!(data["result"]["isError"],true);assert!(data.to_string().contains("demo"));}

#[tokio::test]async fn git_refuses_repository_helpers_without_executing_them(){let f=fixture(|_|{});initialize_git(&f,true);let w=f.rt.project("demo","demo").unwrap();git_cli(&w.root.path,&["config","filter.unsafe.clean","touch SHOULD_NOT_EXIST"]);assert!(git::status(&f.rt,&w).await.is_err());assert!(!w.root.path.join("SHOULD_NOT_EXIST").exists());}

#[tokio::test]
async fn oauth_linking_can_discover_static_tools_before_authorization(){
    let f=fixture(|_|{});let app=server::create_router(f.rt.clone());
    let init=json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"unauthorized-discovery","version":"1"}}});
    let response=http(&app,"POST","/mcp",Body::from(init.to_string()),Some("application/json"),None,None).await;
    assert_eq!(response.status(),StatusCode::OK);
    assert_eq!(json_body(response).await["result"]["serverInfo"]["name"],"EndlessVibe");
    let initialized=json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    assert_eq!(http(&app,"POST","/mcp",Body::from(initialized.to_string()),Some("application/json"),None,None).await.status(),StatusCode::ACCEPTED);
    let list=json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
    for token in [None,Some(f.rt.auth.issue_local_token().unwrap())]{
        let response=http(&app,"POST","/mcp/",Body::from(list.to_string()),Some("application/json"),token.as_deref(),None).await;
        assert_eq!(response.status(),StatusCode::OK);let data=json_body(response).await;
        let tools=data["result"]["tools"].as_array().unwrap();assert_eq!(tools.len(),mcp::TOOL_NAMES.len());
        for tool in tools{
            let name=tool["name"].as_str().unwrap();
            let expected=json!([{"type":"oauth2","scopes":endlessvibe::security::auth::required_scopes(name)}]);
            assert_eq!(tool["securitySchemes"],expected,"{name}");
            assert_eq!(tool["_meta"]["securitySchemes"],expected,"{name}");
        }
        assert!(!data.to_string().contains(&f.owner));
        assert!(!data.to_string().contains(f._dir.path().to_str().unwrap()));
    }
    for path in ["/.well-known/oauth-protected-resource","/.well-known/oauth-protected-resource/mcp"]{
        let data=json_body(http(&app,"GET",path,Body::empty(),None,None,None).await).await;
        assert_eq!(data["resource"],f.rt.config.resource());
        assert_eq!(data["authorization_servers"][0],f.rt.config.server.public_url);
    }
    let meta=json_body(http(&app,"GET","/.well-known/oauth-authorization-server",Body::empty(),None,None,None).await).await;
    assert_eq!(meta["authorization_endpoint"],format!("{}/oauth/authorize",f.rt.config.server.public_url));
    assert_eq!(meta["code_challenge_methods_supported"],json!(["S256"]));
}

#[tokio::test]
async fn every_private_tool_returns_oauth_challenge_without_valid_credentials(){
    let f=fixture(|_|{});let app=server::create_router(f.rt.clone());
    let revoked=f.rt.auth.issue_local_token().unwrap();f.rt.auth.revoke_all().unwrap();
    let expired=f.rt.auth.issue_local_token().unwrap();let key=util::digest(&expired);
    let mut record=f.rt.db.get::<Value>("tokens",&key).unwrap().unwrap();record["expires"]=json!(util::now()-1);
    f.rt.db.put("tokens",&key,&record,0).unwrap();
    for token in [None,Some("invalid"),Some(revoked.as_str()),Some(expired.as_str())]{
        for name in mcp::TOOL_NAMES{
            let call=json!({"jsonrpc":"2.0","id":"link-request","method":"tools/call","params":{"name":name,"arguments":{}}});
            let response=http(&app,"POST","/mcp",Body::from(call.to_string()),Some("application/json"),token,None).await;
            assert_eq!(response.status(),StatusCode::OK,"{name}");
            let header=response.headers()[header::WWW_AUTHENTICATE].to_str().unwrap().to_owned();
            let result=json_body(response).await;
            assert_eq!(result["jsonrpc"],"2.0");assert_eq!(result["id"],"link-request");
            assert_eq!(result["result"]["isError"],true,"{name}");
            assert_eq!(result["result"]["_meta"]["mcp/www_authenticate"][0],header);
            assert!(header.contains("resource_metadata=\"https://mcp.example.com/.well-known/oauth-protected-resource\""));
            assert!(header.contains("error=\"invalid_token\""));assert!(header.contains("error_description="));
            assert!(result["result"]["structuredContent"].is_null());
            assert!(!result.to_string().contains(f._dir.path().to_str().unwrap()));
        }
    }
    assert_eq!(f.rt.jobs.active_count(),0);assert!(f.rt.db.audits(100).unwrap().is_empty());
}

#[tokio::test]
async fn unauthorized_writes_and_mixed_batches_cannot_execute_tools(){
    let f=fixture(|_|{});let app=server::create_router(f.rt.clone());
    let write=json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"write_file","arguments":{"workspace":"demo","project":"demo","path":"forbidden.txt","content":"must not be written","expected_sha256":"MISSING"}}});
    let response=http(&app,"POST","/mcp",Body::from(write.to_string()),Some("application/json"),None,None).await;
    assert_eq!(json_body(response).await["result"]["isError"],true);
    let batch=json!([{"jsonrpc":"2.0","id":2,"method":"tools/list"},write]);
    let token=f.rt.auth.issue_local_token().unwrap();
    for token in [None,Some(token.as_str())]{
        let response=http(&app,"POST","/mcp",Body::from(batch.to_string()),Some("application/json"),token,None).await;
        assert!(response.status().is_client_error());
    }
    for malformed in [json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"write_file","arguments":{"workspace":"demo","project":"demo","path":"forbidden.txt","content":"must not be written","expected_sha256":"MISSING"}}}),json!({"jsonrpc":"2.0","id":null,"method":"tools/call","params":{"name":"hello"}}),json!({"jsonrpc":"1.0","id":1,"method":"tools/list"}),json!({"jsonrpc":"2.0","id":1,"method":"resources/read","params":{"uri":"file:///etc/passwd"}})]{
        assert_eq!(http(&app,"POST","/mcp",Body::from(malformed.to_string()),Some("application/json"),None,None).await.status(),StatusCode::UNAUTHORIZED);
    }
    assert!(!f.rt.project("demo","demo").unwrap().root.path.join("forbidden.txt").exists());
    assert!(f.rt.db.audits(100).unwrap().is_empty());
    assert_eq!(http(&app,"GET","/mcp",Body::empty(),None,None,None).await.status(),StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn browser_consent_preserves_origin_and_rejects_cross_site_or_missing_cookies(){
    let f=fixture(|config|config.server.public_url="https://MCP.EXAMPLE.COM:443".into());
    let app=server::create_router(f.rt.clone());
    let response=http(&app,"POST","/oauth/register",Body::from(json!({"redirect_uris":["https://chatgpt.com/connector_platform_oauth_redirect"]}).to_string()),Some("application/json"),None,None).await;
    assert_eq!(response.status(),StatusCode::CREATED);let client=json_body(response).await["client_id"].as_str().unwrap().to_owned();
    let challenge=URL_SAFE_NO_PAD.encode(Sha256::digest("v".repeat(43).as_bytes()));
    let authorize=format!("/oauth/authorize?{}",form(&[("client_id",&client),("redirect_uri","https://chatgpt.com/connector_platform_oauth_redirect"),("response_type","code"),("state","browser-test"),("code_challenge",&challenge),("code_challenge_method","S256"),("resource",&f.rt.config.resource())]));
    let response=http(&app,"GET",&authorize,Body::empty(),None,None,None).await;
    assert_eq!(response.status(),StatusCode::OK);
    assert_eq!(response.headers()[header::REFERRER_POLICY],"same-origin");
    let cookie=response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_owned();
    let html=String::from_utf8(to_bytes(response.into_body(),32768).await.unwrap().to_vec()).unwrap();
    let transaction=html.split("name=\"transaction\" value=\"").nth(1).unwrap().split('"').next().unwrap();
    let body=form(&[("transaction",transaction),("owner_key",&f.owner),("decision","approve")]);
    for origin in ["null","https://evil.test","https://chatgpt.com","http://mcp.example.com","https://mcp.example.com:444"]{
        let response=consent_http(&app,body.clone(),Some(&cookie),origin).await;
        assert_eq!(response.status(),StatusCode::BAD_REQUEST);
        assert_eq!(json_body(response).await["error_description"],"Cross-origin consent POST refused");
    }
    let origin="https://mcp.example.com";
    for cookie in [None,Some("ev_consent=wrong")]{
        let response=consent_http(&app,body.clone(),cookie,origin).await;
        assert_eq!(response.status(),StatusCode::BAD_REQUEST);
        assert_eq!(json_body(response).await["error_description"],"Consent cookie invalid; restart linking");
    }
    let response=consent_http(&app,body,Some(&cookie),origin).await;
    assert_eq!(response.status(),StatusCode::SEE_OTHER);
    assert_eq!(response.headers()[header::REFERRER_POLICY],"no-referrer");
    let redirect=url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    assert!(redirect.query_pairs().any(|(name,_)|name=="code"));
    assert_eq!(f.rt.db.audits(100).unwrap().len(),1);
    let response=http(&app,"GET","/",Body::empty(),None,None,None).await;
    assert_eq!(response.headers()[header::REFERRER_POLICY],"no-referrer");
}
