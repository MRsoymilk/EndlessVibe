pub mod secure;
pub mod router;
mod idempotency;
use crate::{config::Transfer,store::Store,util};
use anyhow::{bail,Context,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{collections::{BTreeMap,BTreeSet},net::{Ipv4Addr,SocketAddr,SocketAddrV4},sync::{Arc,RwLock},time::Duration};
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
pub struct TransferManager{pub config:Transfer,pub node_id:String,pub db:Arc<Store>,discovered:RwLock<BTreeMap<String,Discovered>>}
// Discover LAN IPv4 interfaces explicitly instead of relying on the OS default
// multicast route (often a VPN or virtual adapter on Windows).
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct DiscoveryInterface {
    ip: Ipv4Addr,
    broadcast: Option<Ipv4Addr>,
}
fn discovery_ipv4(ip:Ipv4Addr)->bool{
    !ip.is_loopback() && !ip.is_unspecified() && (ip.is_private() || ip.is_link_local())
}
fn directed_broadcast(ip:Ipv4Addr,mask:Ipv4Addr,prefixlen:u8,advertised:Option<Ipv4Addr>)->Option<Ipv4Addr>{
    if !(1..=30).contains(&prefixlen){return None;}
    let target=advertised.unwrap_or_else(||Ipv4Addr::from(u32::from(ip)|!u32::from(mask)));
    (target!=ip && target!=Ipv4Addr::BROADCAST && !target.is_unspecified()
        && !target.is_multicast()).then_some(target)
}
fn discovery_interfaces()->Vec<DiscoveryInterface>{
    let all=match if_addrs::get_if_addrs(){
        Ok(items)=>items,
        Err(error)=>{tracing::warn!(error=%error,"Cannot enumerate IPv4 LAN discovery interfaces");return vec![];}
    };
    let mut result=BTreeMap::<Ipv4Addr,DiscoveryInterface>::new();
    for iface in all{
        let if_addrs::IfAddr::V4(v4)=iface.addr else{continue};
        if !discovery_ipv4(v4.ip){continue;}
        let candidate=DiscoveryInterface{ip:v4.ip,
            broadcast:directed_broadcast(v4.ip,v4.netmask,v4.prefixlen,v4.broadcast)};
        result.entry(v4.ip).and_modify(|entry|{
            if entry.broadcast.is_none(){entry.broadcast=candidate.broadcast;}
        }).or_insert(candidate);
    }
    // Bound the amount of traffic on machines with many virtual adapters.
    result.into_values().take(64).collect()
}
fn paired_discovery_addresses(peers:&Value)->Vec<Ipv4Addr>{
    let mut out=BTreeSet::new();
    if let Some(rows)=peers.get("peers").and_then(Value::as_array){
        for row in rows{
            if let Some(std::net::IpAddr::V4(ip))=row.get("endpoint")
                .and_then(Value::as_str)
                .and_then(|address|address.parse::<SocketAddr>().ok())
                .map(|endpoint|endpoint.ip())
            {
                if discovery_ipv4(ip)||ip.is_loopback(){out.insert(ip);}
            }
        }
    }
    out.into_iter().take(128).collect()
}
fn advertise_discovery(payload:&[u8],known_peers:&[Ipv4Addr]){
    let mut multicast_sent=false;
    for iface in discovery_interfaces(){
        let socket=match std::net::UdpSocket::bind(SocketAddrV4::new(iface.ip,0)){
            Ok(socket)=>socket,
            Err(error)=>{tracing::debug!(ip=%iface.ip,error=%error,"Cannot bind discovery interface");continue;}
        };
        let _=socket.set_nonblocking(true);
        if let Err(error)=socket2::SockRef::from(&socket).set_multicast_if_v4(&iface.ip){
            tracing::debug!(ip=%iface.ip,error=%error,"Cannot select multicast interface");
            // The directed broadcast can still work on this interface.
        }else{
            let _=socket.set_multicast_ttl_v4(1);
            match socket.send_to(payload,SocketAddrV4::new(GROUP,DISCOVERY_PORT)){
                Ok(_)=>{multicast_sent=true;},
                Err(error)=>tracing::debug!(ip=%iface.ip,error=%error,"Multicast discovery send failed"),
            }
        }
        if let Some(broadcast)=iface.broadcast{
            let _=socket.set_broadcast(true);
            if let Err(error)=socket.send_to(payload,SocketAddrV4::new(broadcast,DISCOVERY_PORT)){
                tracing::debug!(ip=%iface.ip,broadcast=%broadcast,error=%error,"Directed discovery broadcast failed");
            }
        }
    }
    // Paired peers receive a direct UDP location heartbeat even when multicast
    // and subnet broadcasts are filtered by a router or wireless access point.
    // It carries no trust material and does not bypass TLS or Project grants.
    if let Ok(socket)=std::net::UdpSocket::bind("0.0.0.0:0"){
        let _=socket.set_nonblocking(true);
        for &ip in known_peers{
            if let Err(error)=socket.send_to(payload,SocketAddrV4::new(ip,DISCOVERY_PORT)){
                tracing::debug!(ip=%ip,error=%error,"Paired-peer discovery heartbeat failed");
            }
        }
    }
    // Keep the original behavior as a fallback for unusual adapter layouts.
    if !multicast_sent{
        if let Ok(socket)=std::net::UdpSocket::bind("0.0.0.0:0"){
            let _=socket.set_multicast_ttl_v4(1);
            if let Err(error)=socket.send_to(payload,SocketAddrV4::new(GROUP,DISCOVERY_PORT)){
                tracing::debug!(error=%error,"Fallback multicast discovery send failed");
            }
        }
    }
}
fn refresh_discovery_memberships(socket:&std::net::UdpSocket,joined:&mut BTreeSet<Ipv4Addr>){
    let available=discovery_interfaces().into_iter().map(|i|i.ip).collect::<BTreeSet<_>>();
    for ip in joined.iter().copied().filter(|ip|*ip!=Ipv4Addr::UNSPECIFIED && !available.contains(ip)).collect::<Vec<_>>(){
        let _=socket.leave_multicast_v4(&GROUP,&ip);
        joined.remove(&ip);
    }
    for ip in available {
        if joined.contains(&ip){continue;}
        match socket.join_multicast_v4(&GROUP,&ip){
            Ok(())=>{joined.insert(ip);},
            Err(error)=>tracing::debug!(ip=%ip,error=%error,"Cannot join multicast group on LAN interface"),
        }
    }
    if joined.is_empty(){
        match socket.join_multicast_v4(&GROUP,&Ipv4Addr::UNSPECIFIED){
            Ok(())=>{joined.insert(Ipv4Addr::UNSPECIFIED);},
            Err(error)=>tracing::debug!(error=%error,"Fallback multicast membership unavailable; unicast and broadcast discovery remain active"),
        }
    }
}
fn valid_node_id(id:&str)->bool{id.len()==24&&id.bytes().all(|v|v.is_ascii_hexdigit())}
impl TransferManager{
 pub fn new(config:Transfer,db:Arc<Store>)->Result<Arc<Self>>{let node_id=match db.get::<String>("transfer_meta","node_id")?{Some(id)if valid_node_id(&id)=>id,Some(_)=>bail!("Invalid persisted Transfer Node ID"),None=>{let raw=util::random_secret()?;let id=util::digest(raw);let id=id[..24].to_owned();db.put("transfer_meta","node_id",&id,0)?;id}};Ok(Arc::new(Self{config,node_id,db,discovered:RwLock::new(BTreeMap::new())}))}
 pub fn announce(&self)->Announce{Announce{service:SERVICE.into(),node_id:self.node_id.clone(),name:self.config.display_name.clone(),port:self.config.listen.port()}}
 pub fn observe(&self,packet:&[u8],address:SocketAddr)->Result<bool>{if packet.len()>512{return Ok(false);}let Ok(item)=serde_json::from_slice::<Announce>(packet)else{return Ok(false)};if item.service!=SERVICE||!valid_node_id(&item.node_id)||item.node_id==self.node_id||item.port==0||item.name.is_empty()||item.name.len()>64{return Ok(false);}let address=SocketAddr::new(address.ip(),item.port);let mut known=self.discovered.write().map_err(|_|anyhow::anyhow!("Transfer registry poisoned"))?;if known.len()>=MAX_PEERS&&!known.contains_key(&item.node_id){let oldest=known.iter().min_by_key(|(_,v)|v.last_seen).map(|(k,_)|k.clone());if let Some(k)=oldest{known.remove(&k);}}known.insert(item.node_id.clone(),Discovered{node_id:item.node_id,name:item.name,address,last_seen:util::now(),online:true});Ok(true)}
 pub fn discoveries(&self)->Value{let now=util::now();let devices=self.discovered.read().map(|peers|peers.values().map(|p|{let mut value=serde_json::to_value(p).unwrap_or(Value::Null);value["online"]=json!(now.saturating_sub(p.last_seen)<=20);value}).collect::<Vec<_>>()).unwrap_or_default();json!({"node_id":self.node_id,"name":self.config.display_name,"enabled":self.config.enabled,"listen":self.config.listen,"local_role":self.local_role().unwrap_or("unknown"),"discovery_protocol":SERVICE,"peers":devices})}
 pub async fn run(self:Arc<Self>,shutdown:CancellationToken,rt:Arc<crate::runtime::Runtime>)->Result<()>{
  if !self.config.enabled{return Ok(());}
  let emitter=async{
    if !self.config.advertise{shutdown.cancelled().await;return Ok(());}
    let payload=serde_json::to_vec(&self.announce())?;
    let mut tick=time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    loop{
        tokio::select!{
            _ = shutdown.cancelled()=>break,
            _ = tick.tick()=>{
                let known_peers=match self.peers(){
                    Ok(value)=>paired_discovery_addresses(&value),
                    Err(error)=>{
                        tracing::debug!(error=%error,"Cannot load paired discovery addresses");
                        Vec::new()
                    }
                };
                advertise_discovery(&payload,&known_peers);
            }
        }
    }
    Ok::<_,anyhow::Error>(())
  };
  let receiver=async{
    if !self.config.discover{shutdown.cancelled().await;return Ok(());}
    let socket=std::net::UdpSocket::bind(("0.0.0.0",DISCOVERY_PORT))
        .context("Cannot bind Transfer discovery UDP 20003")?;
    socket.set_nonblocking(true)?;
    let mut joined=BTreeSet::new();
    refresh_discovery_memberships(&socket,&mut joined);
    let receiver=UdpSocket::from_std(socket.try_clone()?)?;
    let mut refresh=time::interval(Duration::from_secs(15));
    refresh.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    let mut bytes=[0u8;1024];
    loop {
        tokio::select!{
            _=shutdown.cancelled()=>break,
            _=refresh.tick()=>refresh_discovery_memberships(&socket,&mut joined),
            result=receiver.recv_from(&mut bytes)=>{
                if let Ok((n,addr))=result{let _=self.observe(&bytes[..n],addr);}
            }
        }
    }
    Ok::<_,anyhow::Error>(())
  };
  // Pairing finalization is automatic: a child only needs to accept or reject.
  // Keep polling even when the parent Dashboard is closed.
  let pairing=async{
    let mut tick=time::interval(Duration::from_secs(3));
    loop{
      tokio::select!{
        _=shutdown.cancelled()=>break,
        _=tick.tick()=>{
          if let Err(error)=self.reconcile_pending().await{
            tracing::debug!(error=%error,"Transfer pairing reconciliation failed");
          }
        }
      }
    }
    Ok::<_,anyhow::Error>(())
  };
  let tls=self.clone().run_tls(shutdown.clone(),Some(rt));
  let(a,b,c,d)=tokio::join!(emitter,receiver,tls,pairing);
  a?;b?;c?;d?;Ok(())
 }
}
#[cfg(test)]mod tests{
use super::*;
#[test]fn paired_discovery_unicast_only_uses_authorized_lan_endpoints(){
    let ips=paired_discovery_addresses(&json!({"peers":[
      {"endpoint":"192.168.1.6:20002"},
      {"endpoint":"192.168.1.6:53241"},
      {"endpoint":"10.0.0.12:20002"},
      {"endpoint":"8.8.8.8:20002"},
      {"endpoint":"invalid"}
    ]}));
    assert_eq!(ips,vec!["10.0.0.12".parse::<Ipv4Addr>().unwrap(),
                        "192.168.1.6".parse::<Ipv4Addr>().unwrap()]);
}
#[test]fn per_interface_discovery_uses_private_addresses_and_directed_broadcast(){
    assert!(discovery_ipv4("192.168.1.6".parse().unwrap()));
    assert!(discovery_ipv4("10.20.30.4".parse().unwrap()));
    assert!(!discovery_ipv4(Ipv4Addr::LOCALHOST));
    assert!(!discovery_ipv4(Ipv4Addr::UNSPECIFIED));
    assert!(!discovery_ipv4("8.8.8.8".parse().unwrap()));
    assert_eq!(directed_broadcast("192.168.1.6".parse().unwrap(),
        "255.255.255.0".parse().unwrap(),24,None),Some("192.168.1.255".parse().unwrap()));
    assert_eq!(directed_broadcast("10.1.2.3".parse().unwrap(),
        "255.255.0.0".parse().unwrap(),16,None),Some("10.1.255.255".parse().unwrap()));
    assert_eq!(directed_broadcast("192.168.1.6".parse().unwrap(),
        "255.255.255.255".parse().unwrap(),32,None),None);
    assert_eq!(directed_broadcast("192.168.1.6".parse().unwrap(),
        "255.255.255.254".parse().unwrap(),31,None),None);
}

#[test]fn transfer_id_persists_and_discovery_is_bounded(){let temp=tempfile::tempdir().unwrap();let db=Arc::new(Store::open(&temp.path().join("db")).unwrap());let first=TransferManager::new(Transfer::default(),db.clone()).unwrap();let second=TransferManager::new(Transfer::default(),db).unwrap();assert_eq!(first.node_id,second.node_id);let announce=Announce{service:SERVICE.into(),node_id:"abcdef123456abcdef123456".into(),name:"Child".into(),port:20002};let msg=serde_json::to_vec(&announce).unwrap();assert!(first.observe(&msg,"192.168.1.99:34567".parse().unwrap()).unwrap());let nodes=first.discoveries();assert_eq!(nodes["peers"][0]["address"],"192.168.1.99:20002");assert_eq!(nodes["peers"][0]["online"],true);assert!(!first.observe(b"{invalid","192.168.1.99:1".parse().unwrap()).unwrap());assert!(!first.observe(&serde_json::to_vec(&first.announce()).unwrap(),"127.0.0.1:100".parse().unwrap()).unwrap());}
}
