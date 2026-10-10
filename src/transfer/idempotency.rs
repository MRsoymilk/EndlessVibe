use crate::{store::Store,util};
use anyhow::{bail,Result};
use serde::{Deserialize,Serialize};
use serde_json::Value;

const NS:&str="transfer_requests";
#[derive(Clone,Serialize,Deserialize)]
struct Record{fingerprint:String,state:String,result:Option<Value>}
pub enum Claim{New,Cached(Value)}
fn key(peer:&str,id:&str)->String{util::digest(format!("{peer}\0{id}"))}
pub fn claim(db:&Store,peer:&str,id:&str,tool:&str,args:&Value)->Result<Claim>{
 if id.is_empty()||id.len()>128||!id.bytes().all(|c|c.is_ascii_alphanumeric()||b"_-".contains(&c)){bail!("Invalid Transfer request_id");}
 let fingerprint=util::digest(format!("{tool}\0{}",args));
 let key=key(peer,id);
 db.transaction(|tx|{
  if let Some(previous)=crate::store::get::<Record>(tx,NS,&key)?{
   if previous.fingerprint!=fingerprint{bail!("Transfer request_id reused with different arguments");}
   return match previous.result{Some(value)=>Ok(Claim::Cached(value)),None=>bail!("Transfer request already started; result uncertain, inspect the target before retrying")};
  }
  crate::store::put(tx,NS,&key,&Record{fingerprint,state:"started".into(),result:None},0)?;
  Ok(Claim::New)
 })
}
pub fn complete(db:&Store,peer:&str,id:&str,result:&Value)->Result<()>{
 let key=key(peer,id);
 db.transaction(|tx|{
  let mut record:Record=crate::store::get(tx,NS,&key)?.ok_or_else(||anyhow::anyhow!("Transfer request record missing"))?;
  record.state="completed".into();record.result=Some(result.clone());
  crate::store::put(tx,NS,&key,&record,0)
 })
}
#[cfg(test)]mod tests{
 use super::*;
 #[test]fn durable_claim_and_conflicts(){
 let t=tempfile::tempdir().unwrap();let path=t.path().join("db");
 {let db=Store::open(&path).unwrap();assert!(matches!(claim(&db,"peer","id","write_file",&serde_json::json!({"a":1})).unwrap(),Claim::New));assert!(claim(&db,"peer","id","write_file",&serde_json::json!({"a":1})).is_err());}
 let db=Store::open(&path).unwrap();
 assert!(claim(&db,"peer","id","write_file",&serde_json::json!({"a":1})).is_err());
 assert!(claim(&db,"peer","id","write_file",&serde_json::json!({"a":2})).is_err());
 complete(&db,"peer","id",&serde_json::json!({"ok":true})).unwrap();
 assert!(matches!(claim(&db,"peer","id","write_file",&serde_json::json!({"a":1})).unwrap(),Claim::Cached(_)));
 assert!(matches!(claim(&db,"other","id","write_file",&serde_json::json!({"a":1})).unwrap(),Claim::New));
 }
}
