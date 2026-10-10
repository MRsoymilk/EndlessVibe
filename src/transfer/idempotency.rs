use crate::{store::Store,util};
use anyhow::{bail,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use rusqlite::params;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

const NS:&str="transfer_requests";
#[derive(Clone,Serialize,Deserialize)]
struct Record{fingerprint:String,state:String,result:Option<Value>,#[serde(default)]workspace:String,#[serde(default)]project:String,#[serde(default)]tool:String,#[serde(default)]error:Option<String>,#[serde(default)]job_id:Option<String>,#[serde(default)]request_id:String,#[serde(default)]peer_node_id:String,#[serde(default)]created:u64,#[serde(default)]updated:u64}
pub enum Claim{New,Cached(Value)}
fn key(peer:&str,id:&str)->String{util::digest(format!("{peer}\0{id}"))}
pub fn claim(db:&Store,peer:&str,id:&str,tool:&str,args:&Value)->Result<Claim>{
 if id.is_empty()||id.len()>128||!id.bytes().all(|c|c.is_ascii_alphanumeric()||b"_-".contains(&c)){bail!("Invalid Transfer request_id");}
 let fingerprint=util::digest(format!("{tool}\0{}",args));
 let key=key(peer,id);
 let workspace=args.get("workspace").and_then(Value::as_str).unwrap_or("");
 let project=args.get("project").and_then(Value::as_str).unwrap_or("");
 let now=util::now();
 db.transaction(|tx|{
  if let Some(previous)=crate::store::get::<Record>(tx,NS,&key)?{
   if previous.fingerprint!=fingerprint{bail!("Transfer request_id reused with different arguments");}
   return match previous.result{Some(value)=>Ok(Claim::Cached(value)),None if previous.state=="result_expired"=>bail!("Transfer request completed but cached response expired; inspect the target, do not replay"),None=>bail!("Transfer request already started; result uncertain, inspect the target before retrying")};
  }
  crate::store::put(tx,NS,&key,&Record{fingerprint,state:"started".into(),result:None,workspace:workspace.into(),project:project.into(),tool:tool.into(),error:None,job_id:None,request_id:id.into(),peer_node_id:peer.into(),created:now,updated:now},0)?;
  Ok(Claim::New)
 })
}
pub fn status(db:&Store,peer:&str,id:&str,workspace:&str,project:&str)->Result<Value>{
 if id.is_empty()||id.len()>128{return Err(anyhow::anyhow!("Invalid Transfer request_id"));}
 let record:Record=db.get(NS,&key(peer,id))?.ok_or_else(||anyhow::anyhow!("Transfer request not found"))?;
 if record.workspace!=workspace||record.project!=project{bail!("Transfer request not found in authorized Project");}
 Ok(serde_json::json!({"request_id":id,"workspace":workspace,"project":project,"tool":record.tool,"state":if record.result.is_some(){"completed"}else if record.state=="failed"{"failed"}else if record.state=="interrupted"{"interrupted"}else if record.state=="result_expired"{"result_expired"}else{"uncertain"},"has_result":record.result.is_some(),"safe_to_replay":false,"error":record.error,"job_id":record.job_id,"recovery_action":if record.result.is_some(){"inspect_recorded_result"}else if record.job_id.is_some(){"inspect_job"}else{"inspect_project_before_new_request"}}))
}
/// A cursor is scoped to the authenticated parent and one Project. It is not authorization.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryCursor {
    scope: String,
    created: u64,
    key: String,
}
fn history_scope(peer: &str, workspace: &str, project: &str) -> String {
    util::digest(format!("{peer}\0{workspace}\0{project}"))
}
fn decode_cursor(raw: &str, scope: &str) -> Result<HistoryCursor> {
    if raw.is_empty() || raw.len() > 512 { bail!("Invalid Transfer history cursor"); }
    let bytes = URL_SAFE_NO_PAD.decode(raw).map_err(|_| anyhow::anyhow!("Invalid Transfer history cursor"))?;
    let cursor: HistoryCursor = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Invalid Transfer history cursor"))?;
    if cursor.scope != scope || cursor.created > i64::MAX as u64 || cursor.key.len() != 64
        || !cursor.key.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("Transfer history cursor is not valid for this Project");
    }
    Ok(cursor)
}
fn encode_cursor(scope: &str, created: u64, key: &str) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&HistoryCursor {
        scope: scope.into(), created, key: key.into(),
    })?))
}
/// Immutable insertion timestamp + hashed key make keyset pages stable across status updates.
pub fn history(db: &Store, peer: &str, workspace: &str, project: &str,
               limit: usize, after: Option<&str>) -> Result<Value> {
    if !(1..=50).contains(&limit) { bail!("Transfer history limit must be 1..50"); }
    let scope = history_scope(peer, workspace, project);
    let cursor = after.map(|token| decode_cursor(token, &scope)).transpose()?;
    db.transaction(|tx| {
        let mut statement = tx.prepare(
            "SELECT key,value FROM kv WHERE namespace=?1
             AND json_extract(value,'$.peer_node_id')=?2
             AND json_extract(value,'$.workspace')=?3
             AND json_extract(value,'$.project')=?4
             AND json_extract(value,'$.request_id') IS NOT NULL
             AND json_extract(value,'$.request_id') != ''
             AND (?5 IS NULL OR CAST(json_extract(value,'$.created') AS INTEGER) < ?5
                  OR (CAST(json_extract(value,'$.created') AS INTEGER) = ?5 AND key < ?6))
             ORDER BY CAST(json_extract(value,'$.created') AS INTEGER) DESC, key DESC
             LIMIT ?7"
        )?;
        let rows = statement.query_map(
            params![NS, peer, workspace, project, cursor.as_ref().map(|c| c.created as i64),
                cursor.as_ref().map(|c| c.key.as_str()), (limit + 1) as i64],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?.collect::<rusqlite::Result<Vec<_>>>()?;
        let has_more = rows.len() > limit;
        let mut requests = Vec::with_capacity(rows.len().min(limit));
        let mut last: Option<(u64, String)> = None;
        for (key, value) in rows.into_iter().take(limit) {
            let r: Record = serde_json::from_str(&value)?;
            last = Some((r.created, key));
            requests.push(json!({
                "request_id":r.request_id, "tool":r.tool,
                "state":if r.result.is_some() {"completed"} else if r.state=="failed" {"failed"}
                    else if r.state=="interrupted" {"interrupted"} else if r.state=="result_expired" {"result_expired"} else {"uncertain"},
                "job_id":r.job_id, "created":r.created, "updated":r.updated,
                "has_result":r.result.is_some(), "safe_to_replay":false,
                "recovery_action":if r.result.is_some() {"inspect_recorded_result"}
                    else {"inspect_project_before_new_request"}
            }));
        }
        let next_cursor = if has_more {
            last.map(|(created, key)| encode_cursor(&scope, created, &key)).transpose()?
        } else { None };
        Ok(json!({"workspace":workspace, "project":project, "requests":requests,
            "limit":limit, "has_more":has_more, "next_cursor":next_cursor}))
    })
}
pub fn fail(db:&Store,peer:&str,id:&str)->Result<()>{let key=key(peer,id);db.transaction(|tx|{let mut record:Record=crate::store::get(tx,NS,&key)?.ok_or_else(||anyhow::anyhow!("Transfer request record missing"))?;if record.result.is_none(){record.state="failed".into();record.error=Some("Operation returned an error; side effects remain uncertain".into());record.updated=util::now();crate::store::put(tx,NS,&key,&record,0)?;}Ok(())})}
pub fn complete(db:&Store,peer:&str,id:&str,result:&Value)->Result<()>{
 let key=key(peer,id);
 db.transaction(|tx|{
  let mut record:Record=crate::store::get(tx,NS,&key)?.ok_or_else(||anyhow::anyhow!("Transfer request record missing"))?;
  record.state="completed".into();record.job_id=result.get("job_id").and_then(Value::as_str).map(str::to_owned);record.result=Some(result.clone());record.updated=util::now();
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
 #[test]
 fn history_is_scoped_bounded_and_omits_saved_outputs(){
  let t=tempfile::tempdir().unwrap();
  let db=Store::open(&t.path().join("db")).unwrap();
  let args=json!({"workspace":"w","project":"p"});
  for id in ["one","two","three"]{
   assert!(matches!(claim(&db,"peer-a",id,"apply_patch",&args).unwrap(),Claim::New));
  }
  complete(&db,"peer-a","one",&json!({"content":"SENSITIVE_RESULT"})).unwrap();
  fail(&db,"peer-a","two").unwrap();
  claim(&db,"peer-b","elsewhere","write_file",&args).unwrap();
  claim(&db,"peer-a","different","write_file",&json!({"workspace":"w","project":"other"})).unwrap();
  let page=history(&db,"peer-a","w","p",2,None).unwrap();
  assert_eq!(page["requests"].as_array().unwrap().len(),2);
  assert_eq!(page["has_more"],true);
  assert!(!page.to_string().contains("SENSITIVE_RESULT"));
  assert!(!page.to_string().contains("peer-b"));
  let full=history(&db,"peer-a","w","p",50,None).unwrap();
  assert_eq!(full["requests"].as_array().unwrap().len(),3);
  assert_eq!(full["has_more"],false);
  assert!(full["requests"].as_array().unwrap().iter().all(|r|r["safe_to_replay"]==false));
  assert!(history(&db,"peer-b","w","p",50,None).unwrap()["requests"].as_array().unwrap().len()==1);
  assert_eq!(history(&db,"peer-a","w","other",50,None).unwrap()["requests"].as_array().unwrap().len(),1);
  assert!(history(&db,"peer-a","w","p",0,None).is_err());
  assert!(history(&db,"peer-a","w","p",51,None).is_err());
 }
 #[test]
 fn keyset_pages_are_duplicate_free_and_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let db = Store::open(&temp.path().join("db")).unwrap();
    let input = json!({"workspace":"w","project":"p"});
    for i in 0..31 {
        let id = format!("id-{i:02}");
        claim(&db,"peer",&id,"apply_patch",&input).unwrap();
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = history(&db,"peer","w","p",7,cursor.as_deref()).unwrap();
        for r in page["requests"].as_array().unwrap() {
            assert!(seen.insert(r["request_id"].as_str().unwrap().to_owned()));
        }
        if !page["has_more"].as_bool().unwrap() {
            assert!(page["next_cursor"].is_null());
            break;
        }
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        assert!(cursor.is_some());
        assert!(history(&db,"other","w","p",7,cursor.as_deref()).is_err());
        assert!(history(&db,"peer","w","other",7,cursor.as_deref()).is_err());
    }
    assert_eq!(seen.len(),31);
    assert!(history(&db,"peer","w","p",7,Some("not-base64!")).is_err());
    assert!(history(&db,"peer","w","p",7,Some(&"a".repeat(513))).is_err());
 }
 #[test]
 fn expired_responses_keep_non_replayable_tombstones() {
    let temp=tempfile::tempdir().unwrap();
    let path=temp.path().join("db");
    let args=json!({"workspace":"w","project":"p"});
    {
        let db=Store::open(&path).unwrap();
        claim(&db,"peer","old-response","write_file",&args).unwrap();
        complete(&db,"peer","old-response",&json!({"content":"SENSITIVE_CACHED_RESULT","job_id":"job-42"})).unwrap();
        claim(&db,"peer","recent-response","write_file",&args).unwrap();
        complete(&db,"peer","recent-response",&json!({"changed":true})).unwrap();
        claim(&db,"peer","uncertain-response","write_file",&args).unwrap();
        let cutoff=util::now().saturating_sub(crate::store::TRANSFER_RESULT_CACHE_SECONDS+3);
        db.transaction(|tx|{
            tx.execute(
                "UPDATE kv SET value=json_set(value,'$.updated',?1) WHERE namespace=?2 AND key=?3",
                params![cutoff,NS,key("peer","old-response")],
            )?;
            Ok(())
        }).unwrap();
        let maintenance=db.maintain(100).unwrap();
        assert_eq!(maintenance["safety"]["transfer_request_tombstones_preserved"],true);
        assert_eq!(maintenance["compacted"]["transfer_response_bodies"],1);
        let tombstone:Value=db.get(NS,&key("peer","old-response")).unwrap().unwrap();
        assert!(tombstone["result"].is_null());
        assert!(!tombstone.to_string().contains("SENSITIVE_CACHED_RESULT"));
        let state=status(&db,"peer","old-response","w","p").unwrap();
        assert_eq!(state["state"],"result_expired");
        assert_eq!(state["job_id"],"job-42");
        assert_eq!(state["safe_to_replay"],false);
        assert_eq!(status(&db,"peer","uncertain-response","w","p").unwrap()["state"],"uncertain");
        assert!(matches!(claim(&db,"peer","recent-response","write_file",&args).unwrap(),Claim::Cached(_)));
        assert!(claim(&db,"peer","old-response","write_file",&args).is_err());
        assert!(claim(&db,"peer","old-response","write_file",&json!({"workspace":"w","project":"other"})).is_err());
    }
    let reopened=Store::open(&path).unwrap();
    assert_eq!(status(&reopened,"peer","old-response","w","p").unwrap()["state"],"result_expired");
    assert!(claim(&reopened,"peer","old-response","write_file",&args).is_err());
 }
 #[test]fn failures_persist_without_allowing_replay(){let t=tempfile::tempdir().unwrap();let db=Store::open(&t.path().join("db")).unwrap();let a=serde_json::json!({"workspace":"w","project":"p"});claim(&db,"peer","failed","apply_patch",&a).unwrap();fail(&db,"peer","failed").unwrap();assert_eq!(status(&db,"peer","failed","w","p").unwrap()["state"],"failed");assert!(claim(&db,"peer","failed","apply_patch",&a).is_err());}
}
