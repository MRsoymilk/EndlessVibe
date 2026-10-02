use anyhow::Result;
use rusqlite::{params,Transaction};

pub const MAX_AUDIT_ROWS:i64=10_000;
pub const MAX_OPERATION_ROWS:i64=5_000;
pub const MAX_TASK_CHECKPOINTS:i64=2_000;
pub const TRAFFIC_RETENTION_SECONDS:u64=7*86_400;

#[derive(Clone,Debug,Default,PartialEq,Eq)]
pub struct RetentionStats{
    pub expired_kv:usize,
    pub task_checkpoints:usize,
    pub audit:usize,
    pub operations:usize,
    pub traffic:usize,
    pub jobs:usize,
}
impl RetentionStats{
    pub fn total(&self)->usize{self.expired_kv+self.task_checkpoints+self.audit+self.operations+self.traffic+self.jobs}
}

pub fn prune_common(tx:&Transaction<'_>,now:u64)->Result<RetentionStats>{
    let expired_kv=tx.execute("DELETE FROM kv WHERE expires>0 AND expires<?1",[now])?;
    let task_checkpoints=tx.execute(
        "DELETE FROM kv WHERE namespace='task_checkpoints' AND key IN (
            SELECT key FROM kv WHERE namespace='task_checkpoints'
            ORDER BY CAST(json_extract(value,'$.updated') AS INTEGER) DESC, rowid DESC
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
    Ok(RetentionStats{expired_kv,task_checkpoints,audit,operations,traffic,jobs:0})
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
