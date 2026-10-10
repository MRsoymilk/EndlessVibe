use crate::{store::Store,util};
use anyhow::{bail,Result};
use serde::{Deserialize,Serialize};
use serde_json::Value;

const NS:&str="transfer_requests";
#[derive(Clone,Serialize,Deserialize)]
struct Record{fingerprint:String,state:String,result:Option<Value>,#[serde(default)]workspace:String,#[serde(default)]project:String,#[serde(default)]tool:String,#[serde(default)]error:Option<String>,#[serde(default)]job_id:Option<String>}
pub enum Claim{New,Cached(Value)}
fn key(peer:&str,id:&str)->String{util::digest(format!("{peer}\0{id}"))}
pub fn claim(db:&Store,peer:&str,id:&str,tool:&str,args:&Value)->Result<Claim>{
 if id.is_empty()||id.len()>128||!id.bytes().all(|c|c.is_ascii_alphanumeric()||b"_-".contains(&c)){bail!("Invalid Transfer request_id");}
 let fingerprint=util::digest(format!("{tool}\0{}",args));
 let key=key(peer,id);
 let workspace=args.get("workspace").and_then(Value::as_str).unwrap_or("");
 let project=args.get("project").and_then(Value::as_str).unwrap_or("");
 db.transaction(|tx|{
  if let Some(previous)=crate::store::get::<Record>(tx,NS,&key)?{
   if previous.fingerprint!=fingerprint{bail!("Transfer request_id reused with different arguments");}
   return match previous.result{Some(value)=>Ok(Claim::Cached(value)),None=>bail!("Transfer request already started; result uncertain, inspect the target before retrying")};
  }
  crate::store::put(tx,NS,&key,&Record{fingerprint,state:"started".into(),result:None,workspace:workspace.into(),project:project.into(),tool:tool.into(),error:None,job_id:None},0)?;
  Ok(Claim::New)
 })
}
pub fn status(db:&Store,peer:&str,id:&str,workspace:&str,project:&str)->Result<Value>{
 if id.is_empty()||id.len()>128{return Err(anyhow::anyhow!("Invalid Transfer request_id"));}
 let record:Record=db.get(NS,&key(peer,id))?.ok_or_else(||anyhow::anyhow!("Transfer request not found"))?;
 if record.workspace!=workspace||record.project!=project{bail!("Transfer request not found in authorized Project");}
 Ok(serde_json::json!({"request_id":id,"workspace":workspace,"project":project,"tool":record.tool,"state":if record.result.is_some(){"completed"}else if record.state=="failed"{"failed"}else if record.state=="interrupted"{"interrupted"}else{"uncertain"},"has_result":record.result.is_some(),"safe_to_replay":false,"error":record.error,"job_id":record.job_id,"recovery_action":if record.result.is_some(){"inspect_recorded_result"}else if record.job_id.is_some(){"inspect_job"}else{"inspect_project_before_new_request"}}))
}
pub fn fail(db:&Store,peer:&str,id:&str)->Result<()>{let key=key(peer,id);db.transaction(|tx|{let mut record:Record=crate::store::get(tx,NS,&key)?.ok_or_else(||anyhow::anyhow!("Transfer request record missing"))?;if record.result.is_none(){record.state="failed".into();record.error=Some("Operation returned an error; side effects remain uncertain".into());crate::store::put(tx,NS,&key,&record,0)?;}Ok(())})}
pub fn complete(db:&Store,peer:&str,id:&str,result:&Value)->Result<()>{
 let key=key(peer,id);
 db.transaction(|tx|{
  let mut record:Record=crate::store::get(tx,NS,&key)?.ok_or_else(||anyhow::anyhow!("Transfer request record missing"))?;
  record.state="completed".into();record.job_id=result.get("job_id").and_then(Value::as_str).map(str::to_owned);record.result=Some(result.clone());
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
 #[test]fn status_is_project_scoped_and_survives_restart(){let t=tempfile::tempdir().unwrap();let path=t.path().join("db");{let db=Store::open(&path).unwrap();claim(&db,"peer","req-1","write_file",&serde_json::json!({"workspace":"w","project":"p"})).unwrap();}let db=Store::open(&path).unwrap();assert_eq!(status(&db,"peer","req-1","w","p").unwrap()["state"],"interrupted");assert!(status(&db,"peer","req-1","w","another").is_err());assert!(status(&db,"another","req-1","w","p").is_err());complete(&db,"peer","req-1",&serde_json::json!({"changed":true})).unwrap();assert_eq!(status(&db,"peer","req-1","w","p").unwrap()["state"],"completed");}
 #[test]fn restarted_incomplete_request_never_replays(){let t=tempfile::tempdir().unwrap();let path=t.path().join("db");let args=serde_json::json!({"workspace":"w","project":"p"});{let db=Store::open(&path).unwrap();claim(&db,"peer","restart","write_file",&args).unwrap();}let db=Store::open(&path).unwrap();let result=status(&db,"peer","restart","w","p").unwrap();assert_eq!(result["state"],"interrupted");assert_eq!(result["safe_to_replay"],false);assert_eq!(result["recovery_action"],"inspect_project_before_new_request");assert!(claim(&db,"peer","restart","write_file",&args).is_err());}
 #[test]fn completed_job_links_job_id(){let t=tempfile::tempdir().unwrap();let db=Store::open(&t.path().join("db")).unwrap();let a=serde_json::json!({"workspace":"w","project":"p"});claim(&db,"peer","job-req","run_command",&a).unwrap();complete(&db,"peer","job-req",&serde_json::json!({"job_id":"job-42","reused":false})).unwrap();assert_eq!(status(&db,"peer","job-req","w","p").unwrap()["job_id"],"job-42");}
 #[test]fn failures_persist_without_allowing_replay(){let t=tempfile::tempdir().unwrap();let db=Store::open(&t.path().join("db")).unwrap();let a=serde_json::json!({"workspace":"w","project":"p"});claim(&db,"peer","failed","apply_patch",&a).unwrap();fail(&db,"peer","failed").unwrap();assert_eq!(status(&db,"peer","failed","w","p").unwrap()["state"],"failed");assert!(claim(&db,"peer","failed","apply_patch",&a).is_err());}
}
