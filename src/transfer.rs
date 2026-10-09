use crate::{config::Transfer,store::Store,util};
use anyhow::{bail,Context,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{collections::BTreeMap,net::{Ipv4Addr,SocketAddr,SocketAddrV4},sync::{Arc,RwLock},time::Duration};
use tokio::{net::UdpSocket,time};
use tokio_util::sync::CancellationToken;

const SERVICE:&str="endlessvibe-transfer-v1";
const GROUP:Ipv4Addr=Ipv4Addr::new(239,255,77,77);
const DISCOVERY_PORT:u16=20003;
const MAX_PEERS:usize=128;
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Announce{pub service:String,pub node_id:String,pub name:String,pub port:u16}
#[derive(Clone,Debug,Serialize)]
pub struct Discovered{pub node_id:String,pub name:String,pub address:SocketAddr,pub last_seen:u64,pub online:bool}
pub struct TransferManager{pub config:Transfer,pub node_id:String,discovered:RwLock<BTreeMap<String,Discovered>>}
fn valid_node_id(id:&str)->bool{id.len()==24&&id.bytes().all(|v|v.is_ascii_hexdigit())}
impl TransferManager{
 pub fn new(config:Transfer,db:&Store)->Result<Arc<Self>>{let node_id=match db.get::<String>("transfer_meta","node_id")?{Some(id)if valid_node_id(&id)=>id,Some(_)=>bail!("Invalid persisted Transfer Node ID"),None=>{let raw=util::random_secret()?;let id=util::digest(raw);let id=id[..24].to_owned();db.put("transfer_meta","node_id",&id,0)?;id}};Ok(Arc::new(Self{config,node_id,discovered:RwLock::new(BTreeMap::new())}))}
 pub fn announce(&self)->Announce{Announce{service:SERVICE.into(),node_id:self.node_id.clone(),name:self.config.display_name.clone(),port:self.config.listen.port()}}
 pub fn observe(&self,packet:&[u8],address:SocketAddr)->Result<bool>{if packet.len()>512{return Ok(false);}let Ok(item)=serde_json::from_slice::<Announce>(packet)else{return Ok(false)};if item.service!=SERVICE||!valid_node_id(&item.node_id)||item.node_id==self.node_id||item.port==0||item.name.is_empty()||item.name.len()>64{return Ok(false);}let address=SocketAddr::new(address.ip(),item.port);let mut known=self.discovered.write().map_err(|_|anyhow::anyhow!("Transfer registry poisoned"))?;if known.len()>=MAX_PEERS&&!known.contains_key(&item.node_id){let oldest=known.iter().min_by_key(|(_,v)|v.last_seen).map(|(k,_)|k.clone());if let Some(k)=oldest{known.remove(&k);}}known.insert(item.node_id.clone(),Discovered{node_id:item.node_id,name:item.name,address,last_seen:util::now(),online:true});Ok(true)}
 pub fn discoveries(&self)->Value{let now=util::now();let devices=self.discovered.read().map(|peers|peers.values().map(|p|{let mut value=serde_json::to_value(p).unwrap_or(Value::Null);value["online"]=json!(now.saturating_sub(p.last_seen)<=20);value}).collect::<Vec<_>>()).unwrap_or_default();json!({"node_id":self.node_id,"name":self.config.display_name,"enabled":self.config.enabled,"listen":self.config.listen,"discovery_protocol":SERVICE,"peers":devices})}
 pub async fn run(self:Arc<Self>,shutdown:CancellationToken)->Result<()>{
  if !self.config.enabled{return Ok(());}
  let emitter=async{if !self.config.advertise{shutdown.cancelled().await;return Ok(());}let socket=UdpSocket::bind("0.0.0.0:0").await?;socket.set_multicast_ttl_v4(1)?;let payload=serde_json::to_vec(&self.announce())?;let mut tick=time::interval(Duration::from_secs(5));loop{tokio::select!{_ = shutdown.cancelled()=>break,_ = tick.tick()=>{if let Err(error)=socket.send_to(&payload,SocketAddrV4::new(GROUP,DISCOVERY_PORT)).await{tracing::debug!(error=%error,"Transfer broadcast unavailable");}}}}Ok::<_,anyhow::Error>(())};
  let receiver=async{if !self.config.discover{shutdown.cancelled().await;return Ok(());}let socket=std::net::UdpSocket::bind(("0.0.0.0",DISCOVERY_PORT)).context("Cannot bind Transfer discovery UDP 20003")?;socket.set_nonblocking(true)?;socket.join_multicast_v4(&GROUP,&Ipv4Addr::UNSPECIFIED)?;let socket=UdpSocket::from_std(socket)?;let mut bytes=[0u8;1024];loop{tokio::select!{_ = shutdown.cancelled()=>break,result=socket.recv_from(&mut bytes)=>{if let Ok((n,addr))=result{let _=self.observe(&bytes[..n],addr);}}}}Ok::<_,anyhow::Error>(())};
  let(a,b)=tokio::join!(emitter,receiver);a?;b?;Ok(())
 }
}
#[cfg(test)]mod tests{
use super::*;
#[test]fn transfer_id_persists_and_discovery_is_bounded(){let temp=tempfile::tempdir().unwrap();let db=Store::open(&temp.path().join("db")).unwrap();let first=TransferManager::new(Transfer::default(),&db).unwrap();let second=TransferManager::new(Transfer::default(),&db).unwrap();assert_eq!(first.node_id,second.node_id);let announce=Announce{service:SERVICE.into(),node_id:"abcdef123456abcdef123456".into(),name:"Child".into(),port:20002};let msg=serde_json::to_vec(&announce).unwrap();assert!(first.observe(&msg,"192.168.1.99:34567".parse().unwrap()).unwrap());let nodes=first.discoveries();assert_eq!(nodes["peers"][0]["address"],"192.168.1.99:20002");assert_eq!(nodes["peers"][0]["online"],true);assert!(!first.observe(b"{invalid","192.168.1.99:1".parse().unwrap()).unwrap());assert!(!first.observe(&serde_json::to_vec(&first.announce()).unwrap(),"127.0.0.1:100".parse().unwrap()).unwrap());}
}
