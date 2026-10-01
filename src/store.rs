use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Serialize};
use std::{path::Path, sync::Mutex, time::Duration};

pub struct Store { connection: Mutex<Connection> }
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.exists() { crate::util::private_create(path, b"")?; }
        {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let f=std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK).open(path)?;
            let m=f.metadata()?;
            if !m.is_file() || m.nlink()!=1 || m.uid()!=unsafe{libc::geteuid()} || m.mode()&0o077!=0 { anyhow::bail!("Database must be an owner-only regular file"); }
        }
        let c = Connection::open(path)?;
        c.busy_timeout(Duration::from_secs(5))?;
        let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0))?; if version>1 { anyhow::bail!("Database belongs to a newer schema; refusing to downgrade"); }
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS kv(namespace TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, expires INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(namespace,key));
            CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY, data TEXT NOT NULL, output BLOB NOT NULL DEFAULT X'', output_offset INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS audit(seq INTEGER PRIMARY KEY AUTOINCREMENT, time INTEGER NOT NULL, tool TEXT NOT NULL, workspace TEXT NOT NULL, outcome TEXT NOT NULL, note TEXT NOT NULL);
            PRAGMA user_version=1;")?;
        Ok(Self { connection: Mutex::new(c) })
    }
    pub fn transaction<T>(&self, op: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut c = self.connection.lock().map_err(|_| anyhow::anyhow!("Database mutex poisoned"))?;
        let tx = c.transaction()?; let out = op(&tx)?; tx.commit()?; Ok(out)
    }
    pub fn get<T: DeserializeOwned>(&self, namespace: &str, key: &str) -> Result<Option<T>> { self.transaction(|tx| get(tx, namespace, key)) }
    pub fn put<T: Serialize>(&self, namespace: &str, key: &str, value: &T, expires: u64) -> Result<()> { self.transaction(|tx| put(tx, namespace, key, value, expires)) }
    pub fn audit(&self, tool: &str, workspace: &str, outcome: &str, note: &str) -> Result<()> {
        self.transaction(|tx| {
            tx.execute("INSERT INTO audit(time,tool,workspace,outcome,note) VALUES(?1,?2,?3,?4,?5)", params![crate::util::now(), tool, workspace, outcome, crate::util::bounded_text(note, 512)])?;
            tx.execute("DELETE FROM audit WHERE seq < (SELECT COALESCE(MAX(seq),0)-10000 FROM audit)", [])?;
            Ok(())
        })
    }
    pub fn audits(&self, limit: usize) -> Result<Vec<serde_json::Value>> {
        self.transaction(|tx| {
            let mut statement = tx.prepare("SELECT seq,time,tool,workspace,outcome,note FROM audit ORDER BY seq DESC LIMIT ?1")?;
            let rows = statement.query_map([limit.min(200) as i64], |r| Ok(serde_json::json!({"seq":r.get::<_,i64>(0)?,"time":r.get::<_,u64>(1)?,"tool":r.get::<_,String>(2)?,"workspace":r.get::<_,String>(3)?,"outcome":r.get::<_,String>(4)?,"note":r.get::<_,String>(5)?})))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
    pub fn dashboard_metrics(&self, window_seconds:u64, bucket_seconds:u64) -> Result<serde_json::Value> {
        let bucket_seconds=bucket_seconds.clamp(10,3600);let buckets=((window_seconds.max(bucket_seconds)+bucket_seconds-1)/bucket_seconds).clamp(1,1440) as usize;
        let now=crate::util::now();let end=(now/bucket_seconds)*bucket_seconds;let start=end.saturating_sub(bucket_seconds.saturating_mul((buckets.saturating_sub(1)) as u64));
        self.transaction(|tx| {
            let mut requests=vec![0u64;buckets];let mut successes=vec![0u64;buckets];let mut failures=vec![0u64;buckets];
            let mut statement=tx.prepare("SELECT time,outcome FROM audit WHERE time>=?1 ORDER BY time")?;
            let rows=statement.query_map([start],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?)))?;
            for row in rows{let(time,outcome)=row?;let index=((time.saturating_sub(start))/bucket_seconds) as usize;if index>=buckets{continue;}match outcome.as_str(){"started"|"accepted"=>requests[index]+=1,"succeeded"=>successes[index]+=1,"failed"|"timed_out"|"cancelled"|"interrupted"=>failures[index]+=1,_=>{}}}
            let mut jobs=Vec::new();let mut q=tx.prepare("SELECT data FROM jobs")?;for row in q.query_map([],|r|r.get::<_,String>(0))?{let value:serde_json::Value=serde_json::from_str(&row?)?;let begin=value.get("started").and_then(|v|v.as_u64()).or_else(||value.get("created").and_then(|v|v.as_u64()));let finish=value.get("finished").and_then(|v|v.as_u64());if let Some(begin)=begin{jobs.push((begin,finish));}}
            let mut points=Vec::with_capacity(buckets);for i in 0..buckets{let time=start+bucket_seconds*i as u64;let sample_time=(time+bucket_seconds.saturating_sub(1)).min(now);let active_jobs=jobs.iter().filter(|(begin,finish)|*begin<=sample_time&&finish.map_or(true,|done|done>sample_time)).count();points.push(serde_json::json!({"time":time,"requests":requests[i],"successes":successes[i],"failures":failures[i],"active_jobs":active_jobs}));}
            Ok(serde_json::json!({"window_seconds":bucket_seconds*buckets as u64,"bucket_seconds":bucket_seconds,"generated_at":now,"totals":{"requests":requests.iter().sum::<u64>(),"successes":successes.iter().sum::<u64>(),"failures":failures.iter().sum::<u64>()},"points":points}))
        })
    }
    pub fn prune_auth(&self) -> Result<()> { self.transaction(|tx| { tx.execute("DELETE FROM kv WHERE expires>0 AND expires<?1", [crate::util::now()])?; Ok(()) }) }
}
pub fn get<T: DeserializeOwned>(tx: &Transaction<'_>, namespace: &str, key: &str) -> Result<Option<T>> {
    let value: Option<String> = tx.query_row("SELECT value FROM kv WHERE namespace=?1 AND key=?2 AND (expires=0 OR expires>=?3)", params![namespace, key, crate::util::now()], |r| r.get(0)).optional()?;
    value.map(|s| serde_json::from_str(&s).context("Invalid stored record")).transpose()
}
pub fn put<T: Serialize>(tx: &Transaction<'_>, namespace: &str, key: &str, value: &T, expires: u64) -> Result<()> {
    tx.execute("INSERT INTO kv(namespace,key,value,expires) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,key) DO UPDATE SET value=excluded.value, expires=excluded.expires", params![namespace, key, serde_json::to_string(value)?, expires])?; Ok(())
}
pub fn delete(tx: &Transaction<'_>, namespace: &str, key: &str) -> Result<()> { tx.execute("DELETE FROM kv WHERE namespace=?1 AND key=?2", params![namespace, key])?; Ok(()) }
pub fn count(tx: &Transaction<'_>, namespace: &str) -> Result<usize> { Ok(tx.query_row("SELECT COUNT(*) FROM kv WHERE namespace=?1", [namespace], |r| r.get::<_,i64>(0))? as usize) }

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn private_storage_persists_and_rolls_back() { let d = tempfile::tempdir().unwrap(); let s = Store::open(&d.path().join("db")).unwrap(); s.put("x", "key", &42, 0).unwrap(); assert_eq!(s.get::<i32>("x", "key").unwrap(), Some(42)); let r: Result<()> = s.transaction(|tx| { put(tx,"x","key",&43,0)?; anyhow::bail!("abort") }); assert!(r.is_err()); assert_eq!(s.get::<i32>("x", "key").unwrap(), Some(42)); }
    #[test] fn dashboard_metrics_aggregate_audit_and_active_jobs(){let d=tempfile::tempdir().unwrap();let s=Store::open(&d.path().join("db")).unwrap();s.audit("read_file","demo/project","started","").unwrap();s.audit("read_file","demo/project","succeeded","").unwrap();let now=crate::util::now();let job=serde_json::json!({"started":now.saturating_sub(1),"finished":null});s.transaction(|tx|{tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2)",rusqlite::params!["job",job.to_string()])?;Ok(())}).unwrap();let value=s.dashboard_metrics(3600,60).unwrap();assert_eq!(value["totals"]["requests"],1);assert_eq!(value["totals"]["successes"],1);assert_eq!(value["totals"]["failures"],0);assert_eq!(value["points"].as_array().unwrap().last().unwrap()["active_jobs"],1);}
}
