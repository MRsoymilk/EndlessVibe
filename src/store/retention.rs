use anyhow::Result;
use rusqlite::{params,OptionalExtension,Transaction};

pub const MAX_AUDIT_ROWS:i64=10_000;
pub const MAX_OPERATION_ROWS:i64=5_000;
pub const MAX_TASK_CHECKPOINTS:i64=2_000;
pub const TRAFFIC_RETENTION_SECONDS:u64=7*86_400;
/// Old successful Transfer responses may be discarded, but request tombstones must never be deleted.
pub const TRANSFER_RESULT_CACHE_SECONDS:u64=30*86_400;
pub const TRANSFER_RESULTS_PER_PASS:i64=200;
const TRANSFER_COMPACT_INTERVAL_SECONDS:u64=3600;

#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub struct RetentionStats{
    pub expired_kv:usize,
    pub task_checkpoints:usize,
    pub audit:usize,
    pub operations:usize,
    pub traffic:usize,
    pub jobs:usize,
    pub transfer_results_compacted:usize,
}
impl RetentionStats{
    pub fn total(&self)->usize{self.expired_kv+self.task_checkpoints+self.audit+self.operations+self.traffic+self.jobs}
}

/// Retire response payloads while retaining the fingerprint and immutable request identity.
pub fn compact_transfer_results(tx:&Transaction<'_>,now:u64)->Result<usize>{
    let cutoff=now.saturating_sub(TRANSFER_RESULT_CACHE_SECONDS).min(i64::MAX as u64) as i64;
    Ok(tx.execute(
        "UPDATE kv SET value=json_set(value,'$.result',json('null'),'$.state','result_expired')
         WHERE namespace='transfer_requests' AND key IN (
           SELECT key FROM kv WHERE namespace='transfer_requests'
           AND json_extract(value,'$.state')='completed'
           AND json_extract(value,'$.result') IS NOT NULL
           AND CAST(json_extract(value,'$.updated') AS INTEGER)>0
           AND CAST(json_extract(value,'$.updated') AS INTEGER)<=?1
           ORDER BY CAST(json_extract(value,'$.updated') AS INTEGER),key
           LIMIT ?2
         )",
        params![cutoff,TRANSFER_RESULTS_PER_PASS]
    )?)
}
fn periodic_transfer_compaction(tx:&Transaction<'_>,now:u64)->Result<usize>{
    let last:Option<u64>=tx.query_row(
        "SELECT CAST(value AS INTEGER) FROM kv WHERE namespace='_maintenance' AND key='transfer_result_last_pass'",
        [],|r|r.get(0)).optional()?;
    if last.is_some_and(|value|now.saturating_sub(value)<TRANSFER_COMPACT_INTERVAL_SECONDS){return Ok(0);}
    let compacted=compact_transfer_results(tx,now)?;
    tx.execute(
        "INSERT INTO kv(namespace,key,value,expires) VALUES('_maintenance','transfer_result_last_pass',?1,0)
         ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value",
        [now.to_string()]
    )?;
    Ok(compacted)
}

pub fn prune_common(tx:&Transaction<'_>,now:u64)->Result<RetentionStats>{
    let transfer_results_compacted=periodic_transfer_compaction(tx,now)?;
    let expired_kv=tx.execute("DELETE FROM kv WHERE expires>0 AND expires<?1",[now])?;
    let task_checkpoints=tx.execute(
        "DELETE FROM kv WHERE namespace='task_checkpoints' AND key IN (
            SELECT key FROM kv WHERE namespace='task_checkpoints'
            ORDER BY CASE
                WHEN COALESCE(json_extract(value,'$.origin'),'explicit')!='auto_job' THEN 0
                WHEN json_extract(value,'$.status') IN ('pending','running') THEN 1
                ELSE 2 END,
                CAST(json_extract(value,'$.updated') AS INTEGER) DESC, rowid DESC
            LIMIT -1 OFFSET ?1
        )",
        [MAX_TASK_CHECKPOINTS],
    )?;
    let audit=tx.execute(
        "DELETE FROM audit WHERE seq IN (
            SELECT seq FROM audit ORDER BY seq DESC LIMIT -1 OFFSET ?1
        )",
        [MAX_AUDIT_ROWS],
    )?;
    let operations=tx.execute(
        "DELETE FROM operation_log WHERE seq IN (
            SELECT seq FROM operation_log
            WHERE status!='running'
            ORDER BY seq DESC
            LIMIT -1 OFFSET ?1
        )",
        [MAX_OPERATION_ROWS],
    )?;
    let traffic=tx.execute(
        "DELETE FROM traffic WHERE bucket<?1",
        [now.saturating_sub(TRAFFIC_RETENTION_SECONDS)],
    )?;
    Ok(RetentionStats{expired_kv,task_checkpoints,audit,operations,traffic,jobs:0,transfer_results_compacted})
}

pub fn prune_jobs(tx:&Transaction<'_>,retained_jobs:usize)->Result<usize>{
    Ok(tx.execute(
        "DELETE FROM jobs WHERE id IN (
            SELECT id FROM jobs
            WHERE json_extract(data,'$.status') NOT IN ('queued','running')
            ORDER BY CAST(json_extract(data,'$.created') AS INTEGER) DESC
            LIMIT -1 OFFSET ?1
        )",
        params![retained_jobs as i64],
    )?)
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn retention_prioritizes_explicit_and_running_stages_over_auto_job_history(){
        let mut c=rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE kv(namespace TEXT,key TEXT,value TEXT,expires INTEGER,PRIMARY KEY(namespace,key));CREATE TABLE jobs(id TEXT PRIMARY KEY,data TEXT,output BLOB DEFAULT X'',output_offset INTEGER DEFAULT 0);CREATE TABLE audit(seq INTEGER PRIMARY KEY AUTOINCREMENT,time INTEGER,tool TEXT,workspace TEXT,outcome TEXT,note TEXT);CREATE TABLE traffic(bucket INTEGER PRIMARY KEY,requests INTEGER,rx_bytes INTEGER,tx_bytes INTEGER);CREATE TABLE operation_log(seq INTEGER PRIMARY KEY AUTOINCREMENT,started INTEGER,finished INTEGER,tool TEXT,workspace TEXT,project TEXT,status TEXT,duration_ms INTEGER,input_json TEXT,output_json TEXT,diff TEXT,added_lines INTEGER,removed_lines INTEGER,error TEXT);").unwrap();
        let tx=c.transaction().unwrap();
        tx.execute("INSERT INTO kv(namespace,key,value,expires) VALUES('task_checkpoints','durable',?1,0)",[serde_json::json!({"origin":"explicit","status":"committed","updated":1}).to_string()]).unwrap();
        tx.execute("INSERT INTO kv(namespace,key,value,expires) VALUES('task_checkpoints','active-auto',?1,0)",[serde_json::json!({"origin":"auto_job","status":"running","updated":1}).to_string()]).unwrap();
        for n in 0..MAX_TASK_CHECKPOINTS+5{tx.execute("INSERT INTO kv(namespace,key,value,expires) VALUES('task_checkpoints',?1,?2,0)",params![format!("auto-{n}"),serde_json::json!({"origin":"auto_job","status":"succeeded","updated":n+10}).to_string()]).unwrap();}
        let stat=prune_common(&tx,MAX_TASK_CHECKPOINTS as u64+20).unwrap();assert_eq!(stat.task_checkpoints,7);
        let remaining:i64=tx.query_row("SELECT COUNT(*) FROM kv WHERE namespace='task_checkpoints'",[],|r|r.get(0)).unwrap();assert_eq!(remaining,MAX_TASK_CHECKPOINTS);
        for key in ["durable","active-auto"]{assert_eq!(tx.query_row("SELECT COUNT(*) FROM kv WHERE namespace='task_checkpoints' AND key=?1",[key],|r|r.get::<_,i64>(0)).unwrap(),1);}
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM kv WHERE namespace='task_checkpoints' AND key='auto-0'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn transfer_response_compaction_is_bounded_and_keeps_tombstones(){
        let mut connection=rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE kv(namespace TEXT,key TEXT,value TEXT,expires INTEGER,
              PRIMARY KEY(namespace,key));"
        ).unwrap();
        let tx=connection.transaction().unwrap();
        for i in 0..(TRANSFER_RESULTS_PER_PASS+3) {
            let data=serde_json::json!({"state":"completed","updated":1,
                "fingerprint":format!("hash-{i}"),"result":{"private":"secret"}});
            tx.execute(
                "INSERT INTO kv(namespace,key,value,expires) VALUES('transfer_requests',?1,?2,0)",
                params![format!("key-{i}"),data.to_string()]
            ).unwrap();
        }
        tx.execute(
            "INSERT INTO kv(namespace,key,value,expires) VALUES('transfer_requests','unfinished',?1,0)",
            [serde_json::json!({"state":"interrupted","updated":1,"result":null}).to_string()]
        ).unwrap();
        let now=TRANSFER_RESULT_CACHE_SECONDS+10;
        assert_eq!(compact_transfer_results(&tx,now).unwrap(),TRANSFER_RESULTS_PER_PASS as usize);
        assert_eq!(compact_transfer_results(&tx,now).unwrap(),3);
        assert_eq!(compact_transfer_results(&tx,now).unwrap(),0);
        let retained:i64=tx.query_row(
            "SELECT COUNT(*) FROM kv WHERE namespace='transfer_requests'",[],|r|r.get(0)
        ).unwrap();
        let expired:i64=tx.query_row(
            "SELECT COUNT(*) FROM kv WHERE namespace='transfer_requests' AND json_extract(value,'$.state')='result_expired'",
            [],|r|r.get(0)
        ).unwrap();
        assert_eq!(retained,TRANSFER_RESULTS_PER_PASS+4);
        assert_eq!(expired,TRANSFER_RESULTS_PER_PASS+3);
        let unresolved:String=tx.query_row(
            "SELECT json_extract(value,'$.state') FROM kv WHERE namespace='transfer_requests' AND key='unfinished'",
            [],|r|r.get(0)
        ).unwrap();
        assert_eq!(unresolved,"interrupted");
    }
    #[test]
    fn retention_preserves_running_operations_and_jobs(){
        let mut c=rusqlite::Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE kv(namespace TEXT,key TEXT,value TEXT,expires INTEGER,PRIMARY KEY(namespace,key));
             CREATE TABLE jobs(id TEXT PRIMARY KEY,data TEXT,output BLOB DEFAULT X'',output_offset INTEGER DEFAULT 0);
             CREATE TABLE audit(seq INTEGER PRIMARY KEY AUTOINCREMENT,time INTEGER,tool TEXT,workspace TEXT,outcome TEXT,note TEXT);
             CREATE TABLE traffic(bucket INTEGER PRIMARY KEY,requests INTEGER,rx_bytes INTEGER,tx_bytes INTEGER);
             CREATE TABLE operation_log(seq INTEGER PRIMARY KEY AUTOINCREMENT,started INTEGER,finished INTEGER,tool TEXT,workspace TEXT,project TEXT,status TEXT,duration_ms INTEGER,input_json TEXT,output_json TEXT,diff TEXT,added_lines INTEGER,removed_lines INTEGER,error TEXT);"
        ).unwrap();
        let tx=c.transaction().unwrap();
        tx.execute("INSERT INTO operation_log(started,tool,workspace,project,status,duration_ms,input_json,output_json,diff,added_lines,removed_lines,error) VALUES(0,'x','w','p','running',0,'{}','{}','',0,0,'')",[]).unwrap();
        tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2)",params!["run",r#"{"status":"running","created":1}"#]).unwrap();
        tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2)",params!["done",r#"{"status":"succeeded","created":0}"#]).unwrap();
        prune_common(&tx,10_000).unwrap();
        prune_jobs(&tx,0).unwrap();
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM operation_log WHERE status='running'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM jobs WHERE id='run'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM jobs WHERE id='done'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }
}
