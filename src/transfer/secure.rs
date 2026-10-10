use super::TransferManager;
use crate::{store::Store,util};
use anyhow::{bail,Context,Result};
use rustls::{client::danger::{ServerCertVerified,ServerCertVerifier,HandshakeSignatureValid},pki_types::{CertificateDer,PrivateKeyDer,PrivatePkcs8KeyDer,ServerName,UnixTime},DigitallySignedStruct,SignatureScheme};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{net::{IpAddr,SocketAddr},sync::Arc,time::Duration};
use tokio::{io::{AsyncRead,AsyncReadExt,AsyncWrite,AsyncWriteExt},net::{TcpListener,TcpStream}};
use tokio_rustls::{TlsAcceptor,TlsConnector};
use tokio_util::sync::CancellationToken;

const MAX_WIRE:usize=2*1024*1024;
const PAIR_TTL:u64=300;
const PAIRS:&str="transfer_peers";
const PENDING:&str="transfer_pending";
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant{pub workspace:String,pub project:String,pub read:bool,pub write:bool,pub execute:bool,pub git:bool}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Peer{pub node_id:String,pub name:String,pub endpoint:SocketAddr,pub cert_sha:String,pub role:String,#[serde(default)]pub token:String,#[serde(default)]pub token_hash:String,#[serde(default)]pub grants:Vec<Grant>,pub paired_at:u64}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct Pending{pub id:String,pub node_id:String,pub name:String,pub endpoint:SocketAddr,pub cert_sha:String,pub token_hash:String,#[serde(default)]pub token:String,pub code:String,pub role:String,pub created:u64}
#[derive(Clone,Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairApprove{pub id:String,#[serde(default)]pub grants:Vec<Grant>}
#[derive(Clone,Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairStart{pub address:SocketAddr}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wire{pub kind:String,pub node_id:String,#[serde(default)]pub name:String,#[serde(default)]pub id:String,#[serde(default)]pub token:String,#[serde(default)]pub tool:String,#[serde(default)]pub args:Value}
#[derive(Debug)]
struct PairVerifier{pin:Option<String>}
impl ServerCertVerifier for PairVerifier{
 fn verify_server_cert(&self,cert:&CertificateDer<'_>,_:&[CertificateDer<'_>],_:&ServerName<'_>,_:&[u8],_:UnixTime)->std::result::Result<ServerCertVerified,rustls::Error>{if self.pin.as_deref().is_some_and(|p|p!=util::digest(cert.as_ref())){return Err(rustls::Error::General("Transfer certificate fingerprint changed".into()));}Ok(ServerCertVerified::assertion())}
 fn verify_tls12_signature(&self,msg:&[u8],cert:&CertificateDer<'_>,dss:&DigitallySignedStruct)->std::result::Result<HandshakeSignatureValid,rustls::Error>{rustls::crypto::verify_tls12_signature(msg,cert,dss,&rustls::crypto::ring::default_provider().signature_verification_algorithms)}
 fn verify_tls13_signature(&self,msg:&[u8],cert:&CertificateDer<'_>,dss:&DigitallySignedStruct)->std::result::Result<HandshakeSignatureValid,rustls::Error>{rustls::crypto::verify_tls13_signature(msg,cert,dss,&rustls::crypto::ring::default_provider().signature_verification_algorithms)}
 fn supported_verify_schemes(&self)->Vec<SignatureScheme>{rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()}
}
fn tls_server_config(db:&Store)->Result<Arc<rustls::ServerConfig>>{
 let pair:Option<(Vec<u8>,Vec<u8>)>=db.get("transfer_meta","tls_identity")?;
 let (cert,key)=if let Some(v)=pair{v}else{let generated=rcgen::generate_simple_self_signed(vec!["endlessvibe.local".into()]).context("Generate Transfer TLS identity")?;let cert=generated.cert.der().to_vec();let key=generated.key_pair.serialize_der();db.put("transfer_meta","tls_identity",&(cert.clone(),key.clone()),0)?;(cert,key)};
 let server=rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_protocol_versions(&[&rustls::version::TLS13])?.with_no_client_auth().with_single_cert(vec![CertificateDer::from(cert)],PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key)))?;Ok(Arc::new(server))
}
fn tls_client_config(pin:Option<String>)->Result<Arc<rustls::ClientConfig>>{let client=rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_protocol_versions(&[&rustls::version::TLS13])?.dangerous().with_custom_certificate_verifier(Arc::new(PairVerifier{pin})).with_no_client_auth();Ok(Arc::new(client))}
async fn send<W:AsyncWrite+Unpin>(stream:&mut W,data:&Value)->Result<()>{let bytes=serde_json::to_vec(data)?;if bytes.len()>MAX_WIRE{bail!("Transfer request exceeds frame limit");}stream.write_u32(bytes.len() as u32).await?;stream.write_all(&bytes).await?;stream.flush().await?;Ok(())}
async fn recv<R:AsyncRead+Unpin>(stream:&mut R)->Result<Value>{let length=stream.read_u32().await? as usize;if length==0||length>MAX_WIRE{bail!("Transfer frame length exceeds limit");}let mut bytes=vec![0u8;length];stream.read_exact(&mut bytes).await?;Ok(serde_json::from_slice(&bytes)?)}
fn code(cert:&str,token:&str)->String{let hash=util::digest(format!("transfer-pair-v1|{cert}|{token}"));let n=u32::from_str_radix(&hash[..8],16).unwrap_or_default()%1_000_000;format!("{n:06}")}
fn valid_node(id:&str)->bool{id.len()==24&&id.bytes().all(|v|v.is_ascii_hexdigit())}
fn lan(address:SocketAddr)->bool{match address.ip(){IpAddr::V4(ip)=>ip.is_private()||ip.is_loopback()||ip.is_link_local(),IpAddr::V6(ip)=>ip.is_loopback()||ip.is_unique_local()||ip.is_unicast_link_local()}}
fn safe_id(id:&str)->bool{!id.is_empty()&&id.len()<=64&&id.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b))}
// Retry only calls without side effects. A lost response to a mutation must never replay it.
fn retryable_tool(tool:&str)->bool{crate::transfer::router::readonly(tool)||matches!(tool,"get_job"|"get_job_output")}
fn public(p:&Pending)->Value{json!({"id":p.id,"node_id":p.node_id,"name":p.name,"endpoint":p.endpoint,"code":p.code,"role":p.role,"expires_at":p.created+PAIR_TTL,"requires_local_confirmation":true})}
async fn request(address:SocketAddr,pin:Option<String>,wire:Value,timeout_seconds:u64)->Result<(Value,String)>{
 if !lan(address){bail!("Transfer only permits private or loopback LAN addresses");}
 let client=TlsConnector::from(tls_client_config(pin)?);
 let connect=async{let stream=TcpStream::connect(address).await?;let mut tls=client.connect(ServerName::try_from("endlessvibe.local")?.to_owned(),stream).await?;let cert=tls.get_ref().1.peer_certificates().and_then(|cs|cs.first()).context("Transfer server did not present a certificate")?;let fingerprint=util::digest(cert.as_ref());send(&mut tls,&wire).await?;let answer=recv(&mut tls).await?;Ok::<_,anyhow::Error>((answer,fingerprint))};
 tokio::time::timeout(Duration::from_secs(timeout_seconds),connect).await.context("Transfer peer connection timed out")?
}
impl TransferManager{
 pub fn pending(&self)->Result<Value>{let now=util::now();let items:Vec<Pending>=self.db.transaction(|tx|{let mut q=tx.prepare("SELECT value FROM kv WHERE namespace=?1 AND (expires=0 OR expires>?2) ORDER BY rowid DESC LIMIT 50")?;let rows=q.query_map(rusqlite::params![PENDING,now],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;rows.into_iter().map(|s|Ok(serde_json::from_str(&s)?)).collect()})?;Ok(json!({"pending":items.iter().map(public).collect::<Vec<_>>()}))}
 pub fn peers(&self)->Result<Value>{let items:Vec<Peer>=self.db.transaction(|tx|{let mut q=tx.prepare("SELECT value FROM kv WHERE namespace=?1 ORDER BY rowid DESC LIMIT 50")?;let rows=q.query_map([PAIRS],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;rows.into_iter().map(|s|Ok(serde_json::from_str(&s)?)).collect()})?;let discovered=self.discoveries();Ok(json!({"peers":items.iter().map(|p|{let seen=discovered["peers"].as_array().and_then(|items|items.iter().find(|d|d["node_id"]==p.node_id));json!({"node_id":p.node_id,"name":p.name,"endpoint":p.endpoint,"role":p.role,"paired_at":p.paired_at,"grants":p.grants,"discovery_online":seen.and_then(|v|v["online"].as_bool()).unwrap_or(false),"discovered_last_seen":seen.and_then(|v|v["last_seen"].as_u64())})}).collect::<Vec<_>>()}))}
 pub async fn start_pair(&self,address:SocketAddr)->Result<Value>{if !self.config.enabled{bail!("Transfer is disabled");}if !lan(address){bail!("Pair target must be a private LAN IP address");}let id=util::digest(util::random_secret()?)[..24].to_owned();let token=util::random_secret()?;let hello=json!({"kind":"pair_hello","node_id":self.node_id,"name":self.config.display_name,"id":id,"token":token});let(v,cert_sha)=request(address,None,hello,12).await?;if v["ok"]!=true{bail!("Pair offer refused: {}",v["error"].as_str().unwrap_or("unknown"));}let peer=v["node_id"].as_str().context("Pair response lacked node ID")?;if !valid_node(peer)||peer==self.node_id{bail!("Invalid remote Node ID");}let pairing_code=code(&cert_sha,&token);if v["code"].as_str()!=Some(&pairing_code){bail!("Pair verification code mismatch");}let pending=Pending{id,node_id:peer.into(),name:v["name"].as_str().unwrap_or("Remote").to_owned(),endpoint:address,cert_sha,token_hash:util::digest(&token),token,code:pairing_code,role:"parent".into(),created:util::now()};self.db.put(PENDING,&pending.id,&pending,pending.created+PAIR_TTL)?;Ok(public(&pending))}
 pub fn receive_pair(&self,wire:&Wire,address:SocketAddr,cert_sha:&str)->Result<Value>{if !valid_node(&wire.node_id)||wire.node_id==self.node_id||wire.id.len()!=24||wire.token.len()<32||wire.name.is_empty()||wire.name.len()>64{bail!("Invalid Transfer pair offer");}if let Some(existing)=self.db.get::<String>("transfer_meta","parent_node")?{if existing!=wire.node_id{bail!("This child is already paired with a different parent");}}let item=Pending{id:wire.id.clone(),node_id:wire.node_id.clone(),name:wire.name.clone(),endpoint:address,cert_sha:cert_sha.into(),token_hash:util::digest(&wire.token),token:String::new(),code:code(cert_sha,&wire.token),role:"child".into(),created:util::now()};self.db.put(PENDING,&item.id,&item,item.created+PAIR_TTL)?;Ok(json!({"ok":true,"node_id":self.node_id,"name":self.config.display_name,"code":item.code}))}
 pub fn approve_pair(&self,request:PairApprove,rt:&crate::runtime::Runtime)->Result<Value>{let p:Pending=self.db.get(PENDING,&request.id)?.context("Pair request is missing or expired")?;if util::now().saturating_sub(p.created)>PAIR_TTL{bail!("Pair request expired");}if p.role=="child"{if request.grants.is_empty(){bail!("Child must explicitly grant at least one Project");}if request.grants.len()>32{bail!("Too many Project grants");}for g in &request.grants{if !safe_id(&g.workspace)||!safe_id(&g.project){bail!("Invalid Project grant");}let project=rt.project_exact(&g.workspace,&g.project)?;if (g.write||g.execute||g.git)&&!project.config.allow_write||(g.execute&&!project.config.allow_exec)||(g.git&&!project.config.allow_git_commit){bail!("Grant exceeds the child's existing Project permissions");}}if let Some(existing)=self.db.get::<String>("transfer_meta","parent_node")?{if existing!=p.node_id{bail!("Child already paired with a different parent");}}}
 let peer=Peer{node_id:p.node_id.clone(),name:p.name.clone(),endpoint:p.endpoint,cert_sha:p.cert_sha,role:p.role.clone(),token:p.token,token_hash:p.token_hash,grants:if p.role=="child"{request.grants}else{vec![]},paired_at:util::now()};self.db.transaction(|tx|{if peer.role=="child"{crate::store::put(tx,"transfer_meta","parent_node",&peer.node_id,0)?;}crate::store::put(tx,PAIRS,&peer.node_id,&peer,0)?;crate::store::delete(tx,PENDING,&request.id)?;Ok(())})?;Ok(json!({"paired":true,"node_id":peer.node_id,"role":peer.role,"grants":peer.grants}))}
 pub fn revoke_pair(&self,node_id:&str)->Result<Value>{if !valid_node(node_id){bail!("Invalid Node ID");}self.db.transaction(|tx|{let item:Option<Peer>=crate::store::get(tx,PAIRS,node_id)?;if let Some(ref p)=item{if p.role=="child"{crate::store::delete(tx,"transfer_meta","parent_node")?;}crate::store::delete(tx,PAIRS,node_id)?;}Ok(json!({"revoked":item.is_some(),"node_id":node_id}))})}
 pub fn incoming_peer(&self,node_id:&str,token:&str)->Result<Peer>{let peer:Peer=self.db.get(PAIRS,node_id)?.context("Transfer peer is not paired")?;if peer.role!="child"{bail!("This peer cannot control local Projects");}let hash=util::digest(token);if token.len()<32||!bool::from(subtle::ConstantTimeEq::ct_eq(hash.as_bytes(),peer.token_hash.as_bytes())){bail!("Invalid Transfer peer credential");}Ok(peer)}
 pub async fn call_node(&self,node_id:&str,tool:&str,args:Value)->Result<Value>{self.call_node_with_request_id(node_id,tool,args,None).await}
 pub async fn call_node_with_request_id(&self,node_id:&str,tool:&str,args:Value,request_id:Option<&str>)->Result<Value>{if !self.config.enabled{bail!("Transfer is disabled");}if !valid_node(node_id){bail!("Invalid target Node ID");}let peer:Peer=self.db.get(PAIRS,node_id)?.context("Node is not paired")?;if peer.role!="parent"||peer.token.is_empty(){bail!("Only an approved parent may forward Project calls to a child");}let id=if let Some(id)=request_id{id.to_owned()}else if tool=="run_command"{args.get("request_id").and_then(Value::as_str).map(str::to_owned).unwrap_or(util::random_secret()?)}else{util::random_secret()?};let req=json!({"kind":"call","node_id":self.node_id,"token":peer.token,"tool":tool,"id":id,"args":args});let first=request(peer.endpoint,Some(peer.cert_sha.clone()),req.clone(),90).await;
 let(answer,fingerprint)=match first{Ok(result)=>result,Err(error) if retryable_tool(tool)=>{tracing::debug!(node_id=%node_id,tool,error=%error,"Retrying safe Transfer read after connection failure");request(peer.endpoint,Some(peer.cert_sha.clone()),req,90).await?},Err(error)=>return Err(error)};
 if fingerprint!=peer.cert_sha{bail!("Transfer child certificate changed");}if answer["ok"]!=true{bail!("Child refused transfer request: {}",answer["error"].as_str().unwrap_or("unknown"));}Ok(answer["result"].clone())}
 pub fn authorized(&self,node_id:&str,token:&str,workspace:&str,project:&str,operation:&str)->Result<Grant>{let peer=self.incoming_peer(node_id,token)?;let grant=peer.grants.into_iter().find(|g|g.workspace==workspace&&g.project==project).context("Project not granted to parent")?;let allowed=match operation{"read"=>grant.read,"write"=>grant.write,"execute"=>grant.execute,"git"=>grant.git,_=>false};if !allowed{bail!("Transfer operation not granted");}Ok(grant)}
 pub async fn run_tls(self:Arc<Self>,shutdown:CancellationToken,rt:Option<Arc<crate::runtime::Runtime>>)->Result<()>{
  let server=tls_server_config(&self.db)?;
  let listener=TcpListener::bind(self.config.listen).await
   .with_context(||format!("Cannot bind Transfer TLS {}",self.config.listen))?;
  let acceptor=TlsAcceptor::from(server);
  let slots=Arc::new(tokio::sync::Semaphore::new(12));
  // Reap completed TLS sessions while serving; detached tasks must not outlive shutdown.
  let mut sessions=tokio::task::JoinSet::new();
  loop{
   tokio::select!{
    _=shutdown.cancelled()=>break,
    completed=sessions.join_next(),if !sessions.is_empty()=>{
     if let Some(Err(error))=completed{tracing::debug!(error=%error,"Transfer TLS session join failed");}
    },
    connection=listener.accept()=>{
     let(stream,addr)=connection?;
     if !lan(addr){continue;}
     let Ok(permit)=slots.clone().try_acquire_owned()else{continue;};
     let manager=self.clone();let tls=acceptor.clone();let rt=rt.clone();
     sessions.spawn(async move{
      let _permit=permit;
      let fut=async{
       let mut session=tls.accept(stream).await?;
       let request=recv(&mut session).await?;
       let wire:Wire=serde_json::from_value(request)?;
       let cert:Vec<u8>=manager.db.get::<(Vec<u8>,Vec<u8>)>("transfer_meta","tls_identity")?
        .context("Missing server identity")?.0;
       let digest=util::digest(&cert);
       let response=match wire.kind.as_str(){
        "pair_hello"=>manager.receive_pair(&wire,addr,&digest),
        "call"=>{
         let rt=rt.context("Transfer Project routing is not active")?;
         crate::transfer::router::dispatch(rt,manager.clone(),wire)
          .await.map(|outcome|json!({"ok":true,"node_id":manager.node_id,"result":outcome}))
        },
        _=>Err(anyhow::anyhow!("Transfer request type is not supported"))
       };
       let data=match response{
        Ok(value)=>value,
        Err(error)=>json!({"ok":false,"error":util::bounded_text(&error.to_string(),200)})
       };
       send(&mut session,&data).await?;
       Ok::<_,anyhow::Error>(())
      };
      if let Err(error)=tokio::time::timeout(Duration::from_secs(95),fut)
       .await.unwrap_or_else(|_|Err(anyhow::anyhow!("Peer request timed out"))){
       tracing::debug!(error=%error,"Transfer TLS session failed");
      }
     });
    }
   }
  }
  // Allow active requests to finish before releasing Runtime; terminate stalled
  // half-open sessions after a bounded grace period.
  if tokio::time::timeout(Duration::from_secs(3),async{
   while let Some(joined)=sessions.join_next().await{
    if let Err(error)=joined{tracing::debug!(error=%error,"Transfer TLS session join failed");}
   }
  }).await.is_err(){
   tracing::debug!("Transfer shutdown aborting sessions that exceeded grace period");
   sessions.abort_all();
   while sessions.join_next().await.is_some(){}
  }
  Ok(())
 }
}
#[cfg(test)]mod tests{use super::*;
#[tokio::test]async fn tls_pair_offer_matches_both_nodes_and_never_grants_projects(){let a=tempfile::tempdir().unwrap();let b=tempfile::tempdir().unwrap();let db_a=Arc::new(Store::open(&a.path().join("state")).unwrap());let db_b=Arc::new(Store::open(&b.path().join("state")).unwrap());let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();drop(listener);let mut child_cfg=crate::config::Transfer::default();child_cfg.enabled=true;child_cfg.listen=address;child_cfg.display_name="Child".into();let child=TransferManager::new(child_cfg,db_b).unwrap();let mut parent_cfg=crate::config::Transfer::default();parent_cfg.enabled=true;let parent=TransferManager::new(parent_cfg,db_a).unwrap();let shutdown=CancellationToken::new();let task=tokio::spawn(child.clone().run_tls(shutdown.clone(),None));tokio::time::sleep(Duration::from_millis(100)).await;let result=parent.start_pair(address).await.unwrap();assert_eq!(result["role"],"parent");let pending=child.pending().unwrap();assert_eq!(pending["pending"][0]["code"],result["code"]);assert_eq!(pending["pending"][0]["node_id"],parent.node_id);assert_eq!(child.peers().unwrap()["peers"].as_array().unwrap().len(),0);assert!(child.authorized(&parent.node_id,"invalid","w","p","read").is_err());shutdown.cancel();task.await.unwrap().unwrap();}

#[tokio::test]
async fn tls_partial_frame_and_disconnected_response_never_duplicate_mutation(){
 use crate::config::{Config,WorkspaceConfig,ProjectConfig};
 use crate::runtime::Runtime;
 let child_dir=tempfile::tempdir().unwrap();
 let parent_dir=tempfile::tempdir().unwrap();
 let probe=TcpListener::bind("127.0.0.1:0").await.unwrap();
 let address=probe.local_addr().unwrap();drop(probe);
 let root=child_dir.path().join("workspace");
 std::fs::create_dir_all(root.join("project")).unwrap();
 let mut config=Config::default();
 config.security.data_dir=child_dir.path().join("private_state");
 config.execution.backend="host".into();
 config.execution.acknowledge_unsafe_host_execution=true;
 config.transfer.enabled=true;config.transfer.listen=address;
 config.workspaces=vec![WorkspaceConfig{id:"demo".into(),path:root.clone(),
  projects:vec![ProjectConfig{id:"demo".into(),path:"project".into(),
   allow_write:true,allow_exec:true,allow_git_commit:true,allow_git_mutation:false,
   allow_git_push:false,execution_profile:crate::config::default_project_profile(),
   environment:vec![]}],allow_write:None,allow_exec:None,allow_git_commit:None,
   allow_git_mutation:None,allow_git_push:None,execution_profile:None,environment:vec![]}];
 util::private_dir(&config.security.data_dir).unwrap();
 util::private_create(&config.security.data_dir.join("owner.key"),util::random_secret().unwrap().as_bytes()).unwrap();
 let config_path=child_dir.path().join("config.toml");
 util::private_create(&config_path,toml::to_string(&config).unwrap().as_bytes()).unwrap();
 let rt=Runtime::new(config,&config_path).unwrap();
 let db=Arc::new(Store::open(&parent_dir.path().join("state")).unwrap());
 let mut parent_cfg=crate::config::Transfer::default();parent_cfg.enabled=true;
 let parent=TransferManager::new(parent_cfg,db).unwrap();
 let shutdown=CancellationToken::new();
 let server=tokio::spawn(rt.transfer.clone().run_tls(shutdown.clone(),Some(rt.clone())));
 tokio::time::sleep(Duration::from_millis(100)).await;
 let offer=parent.start_pair(address).await.unwrap();
 let id=offer["id"].as_str().unwrap().to_owned();
 rt.transfer.approve_pair(PairApprove{id:id.clone(),grants:vec![Grant{
  workspace:"demo".into(),project:"demo".into(),read:true,write:true,execute:false,git:false
 }]},&rt).unwrap();
 parent.approve_pair(PairApprove{id,grants:vec![]},&rt).unwrap();
 let peer:Peer=parent.db.get(PAIRS,&rt.transfer.node_id).unwrap().unwrap();
 let connector=TlsConnector::from(tls_client_config(Some(peer.cert_sha.clone())).unwrap());

 // A valid TLS session that drops midway through a length-prefixed JSON message
 // must never reach dispatch or create any durable request claim.
 let sock=TcpStream::connect(address).await.unwrap();
 let mut partial=connector.connect(ServerName::try_from("endlessvibe.local").unwrap().to_owned(),sock).await.unwrap();
 partial.write_u32(128).await.unwrap();
 partial.write_all(b"{\"kind\":\"call\"").await.unwrap();
 partial.flush().await.unwrap();
 drop(partial);
 tokio::time::sleep(Duration::from_millis(30)).await;
 let count=rt.db.transaction(|tx|Ok(tx.query_row(
  "SELECT COUNT(*) FROM operation_log WHERE tool LIKE 'transfer_%'",[],|r|r.get::<_,i64>(0))?)).unwrap();
 assert_eq!(count,0);

 // Send a complete authenticated write and physically close TCP without ever
 // reading the reply. The child may have executed it; replay is never allowed.
 let arguments=json!({"workspace":"demo","project":"demo","path":"created-once"});
 let request_id="connection-dropped-after-send";
 let wire=json!({"kind":"call","node_id":parent.node_id,"token":peer.token,
  "tool":"create_directory","id":request_id,"args":arguments});
 let sock=TcpStream::connect(address).await.unwrap();
 let mut disconnected=connector.connect(ServerName::try_from("endlessvibe.local").unwrap().to_owned(),sock).await.unwrap();
 send(&mut disconnected,&wire).await.unwrap();
 drop(disconnected); // Deliberately do not call recv.
 let key=util::digest(format!("{}\0{}",parent.node_id,request_id));
 tokio::time::timeout(Duration::from_secs(3),async{
  loop{
   if let Some(r)=rt.db.get::<Value>("transfer_requests",&key).unwrap(){
    if r["state"]=="completed"{break;}
   }
   tokio::time::sleep(Duration::from_millis(10)).await;
  }
 }).await.expect("Child did not durably finish a write after client dropped TCP");
 assert!(root.join("project/created-once").is_dir());
 let before=rt.db.transaction(|tx|Ok(tx.query_row(
  "SELECT COUNT(*) FROM operation_log WHERE tool='transfer_create_directory'",[],|r|r.get::<_,i64>(0))?)).unwrap();
 assert_eq!(before,1);
 let answer=parent.call_node_with_request_id(&rt.transfer.node_id,"create_directory",
  json!({"workspace":"demo","project":"demo","path":"created-once"}),Some(request_id)).await.unwrap();
 assert!(answer.is_object());
 let after=rt.db.transaction(|tx|Ok(tx.query_row(
  "SELECT COUNT(*) FROM operation_log WHERE tool='transfer_create_directory'",[],|r|r.get::<_,i64>(0))?)).unwrap();
 assert_eq!(after,before);
 shutdown.cancel();server.await.unwrap().unwrap();
}

#[tokio::test]
async fn tls_shutdown_drains_or_aborts_stalled_partial_frames(){
 let dir=tempfile::tempdir().unwrap();
 let db=Arc::new(Store::open(&dir.path().join("db")).unwrap());
 let probe=TcpListener::bind("127.0.0.1:0").await.unwrap();
 let address=probe.local_addr().unwrap();drop(probe);
 let mut cfg=crate::config::Transfer::default();
 cfg.enabled=true;cfg.listen=address;
 let manager=TransferManager::new(cfg,db).unwrap();
 let shutdown=CancellationToken::new();
 let task=tokio::spawn(manager.clone().run_tls(shutdown.clone(),None));
 tokio::time::sleep(Duration::from_millis(100)).await;
 let connector=TlsConnector::from(tls_client_config(None).unwrap());
 let socket=TcpStream::connect(address).await.unwrap();
 let mut connection=connector.connect(
  ServerName::try_from("endlessvibe.local").unwrap().to_owned(),socket
 ).await.unwrap();
 // Keep a TLS session open after claiming a frame larger than the bytes sent.
 connection.write_u32(4096).await.unwrap();
 connection.write_all(b"{\"kind\":").await.unwrap();
 connection.flush().await.unwrap();
 shutdown.cancel();
 tokio::time::timeout(Duration::from_secs(6),task).await
  .expect("TLS listener shutdown exceeded bounded grace").unwrap().unwrap();
 // The partial request cannot outlive the listener and eventually execute.
 let outcome=tokio::time::timeout(Duration::from_secs(1),recv(&mut connection)).await
  .expect("Stalled TLS session was not closed after shutdown");
 assert!(outcome.is_err());
}
#[test]fn retry_policy_never_replays_mutations(){for tool in ["read_file","list_directory","git_status","get_task_checkpoint","continue_task","get_job","get_job_output"]{assert!(retryable_tool(tool),"{tool}");}for tool in ["write_file","apply_patch","create_directory","run_command","cancel_job","git_commit","git_push","run_shell"]{assert!(!retryable_tool(tool),"{tool}");}}
#[test]fn code_is_stable_and_permissions_fail_closed(){assert_eq!(code("cert","token"),code("cert","token"));assert_ne!(code("cert","token"),code("cert2","token"));assert!(!lan("8.8.8.8:20002".parse().unwrap()));assert!(lan("127.0.0.1:20002".parse().unwrap()));}}
