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
            CREATE TABLE IF NOT EXISTS traffic(bucket INTEGER PRIMARY KEY, requests INTEGER NOT NULL DEFAULT 0, rx_bytes INTEGER NOT NULL DEFAULT 0, tx_bytes INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS operation_log(seq INTEGER PRIMARY KEY AUTOINCREMENT, started INTEGER NOT NULL, finished INTEGER, tool TEXT NOT NULL, workspace TEXT NOT NULL, project TEXT NOT NULL, status TEXT NOT NULL, duration_ms INTEGER NOT NULL DEFAULT 0, input_json TEXT NOT NULL, output_json TEXT NOT NULL DEFAULT '{}', diff TEXT NOT NULL DEFAULT '', added_lines INTEGER NOT NULL DEFAULT 0, removed_lines INTEGER NOT NULL DEFAULT 0, error TEXT NOT NULL DEFAULT '');
            PRAGMA user_version=1;")?;
        c.execute("UPDATE operation_log SET status='interrupted',finished=?1,error=CASE WHEN error='' THEN 'Service restarted before operation completed' ELSE error END WHERE status='running'",[crate::util::now()])?;
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
    pub fn operation_start(&self,tool:&str,workspace:&str,project:&str,input:&serde_json::Value)->Result<i64>{
        let input_json=bounded_operation_json(input.clone());
        self.transaction(|tx|{tx.execute("INSERT INTO operation_log(started,tool,workspace,project,status,input_json) VALUES(?1,?2,?3,?4,'running',?5)",params![crate::util::now(),tool,workspace,project,input_json])?;Ok(tx.last_insert_rowid())})
    }
    pub fn operation_finish(&self,seq:i64,status:&str,duration_ms:u64,output:Option<&serde_json::Value>,diff:&str,error:&str)->Result<()>{
        let output_json=output.map(|v|bounded_operation_json(v.clone())).unwrap_or_else(||"{}".into());let diff=bounded_operation_text(diff,262144);let(added_lines,removed_lines)=diff_stats(&diff);let error=bounded_operation_text(error,8192);
        self.transaction(|tx|{tx.execute("UPDATE operation_log SET finished=?2,status=?3,duration_ms=?4,output_json=?5,diff=?6,added_lines=?7,removed_lines=?8,error=?9 WHERE seq=?1",params![seq,crate::util::now(),status,duration_ms,output_json,diff,added_lines,removed_lines,error])?;tx.execute("DELETE FROM operation_log WHERE seq < (SELECT COALESCE(MAX(seq),0)-5000 FROM operation_log)",[])?;Ok(())})
    }
    pub fn operations(&self,limit:usize)->Result<Vec<serde_json::Value>>{
        self.transaction(|tx|{let mut q=tx.prepare("SELECT seq,started,finished,tool,workspace,project,status,duration_ms,added_lines,removed_lines,error FROM operation_log ORDER BY seq DESC LIMIT ?1")?;let rows=q.query_map([limit.clamp(1,200) as i64],|r|Ok(serde_json::json!({"seq":r.get::<_,i64>(0)?,"started":r.get::<_,u64>(1)?,"finished":r.get::<_,Option<u64>>(2)?,"tool":r.get::<_,String>(3)?,"workspace":r.get::<_,String>(4)?,"project":r.get::<_,String>(5)?,"status":r.get::<_,String>(6)?,"duration_ms":r.get::<_,u64>(7)?,"added_lines":r.get::<_,u64>(8)?,"removed_lines":r.get::<_,u64>(9)?,"error":r.get::<_,String>(10)?})))?;Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)})
    }
    pub fn operation(&self,seq:i64)->Result<serde_json::Value>{
        self.transaction(|tx|{let value=tx.query_row("SELECT seq,started,finished,tool,workspace,project,status,duration_ms,input_json,output_json,diff,added_lines,removed_lines,error FROM operation_log WHERE seq=?1",[seq],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,u64>(1)?,r.get::<_,Option<u64>>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,u64>(7)?,r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,String>(10)?,r.get::<_,u64>(11)?,r.get::<_,u64>(12)?,r.get::<_,String>(13)?))).optional()?.context("Operation not found")?;let input=serde_json::from_str::<serde_json::Value>(&value.8).unwrap_or_else(|_|serde_json::json!({"raw":value.8}));let output=serde_json::from_str::<serde_json::Value>(&value.9).unwrap_or_else(|_|serde_json::json!({"raw":value.9}));Ok(serde_json::json!({"seq":value.0,"started":value.1,"finished":value.2,"tool":value.3,"workspace":value.4,"project":value.5,"status":value.6,"duration_ms":value.7,"input":input,"output":output,"diff":value.10,"added_lines":value.11,"removed_lines":value.12,"error":value.13}))})
    }
    pub fn record_traffic(&self,requests:u64,rx_bytes:u64,tx_bytes:u64)->Result<()> {
        if requests==0&&rx_bytes==0&&tx_bytes==0{return Ok(());}let now=crate::util::now();let bucket=now/60*60;
        self.transaction(|tx|{
            tx.execute("INSERT INTO traffic(bucket,requests,rx_bytes,tx_bytes) VALUES(?1,?2,?3,?4) ON CONFLICT(bucket) DO UPDATE SET requests=requests+excluded.requests,rx_bytes=rx_bytes+excluded.rx_bytes,tx_bytes=tx_bytes+excluded.tx_bytes",params![bucket,requests,rx_bytes,tx_bytes])?;
            tx.execute("DELETE FROM traffic WHERE bucket<?1",[now.saturating_sub(7*86400)])?;Ok(())
        })
    }
    pub fn dashboard_metrics(&self, window_seconds:u64, bucket_seconds:u64) -> Result<serde_json::Value> {
        let bucket_seconds=bucket_seconds.clamp(10,3600);let buckets=((window_seconds.max(bucket_seconds)+bucket_seconds-1)/bucket_seconds).clamp(1,1440) as usize;
        let now=crate::util::now();let end=(now/bucket_seconds)*bucket_seconds;let start=end.saturating_sub(bucket_seconds.saturating_mul((buckets.saturating_sub(1)) as u64));
        self.transaction(|tx| {
            let mut requests=vec![0u64;buckets];let mut successes=vec![0u64;buckets];let mut failures=vec![0u64;buckets];let mut http_requests=vec![0u64;buckets];let mut rx_bytes=vec![0u64;buckets];let mut tx_bytes=vec![0u64;buckets];
            let mut statement=tx.prepare("SELECT time,outcome FROM audit WHERE time>=?1 ORDER BY time")?;
            let rows=statement.query_map([start],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,String>(1)?)))?;
            for row in rows{let(time,outcome)=row?;let index=((time.saturating_sub(start))/bucket_seconds) as usize;if index>=buckets{continue;}match outcome.as_str(){"started"|"accepted"=>requests[index]+=1,"succeeded"=>successes[index]+=1,"failed"|"timed_out"|"cancelled"|"interrupted"=>failures[index]+=1,_=>{}}}
            let mut traffic=tx.prepare("SELECT bucket,requests,rx_bytes,tx_bytes FROM traffic WHERE bucket>=?1 ORDER BY bucket")?;for row in traffic.query_map([start],|r|Ok((r.get::<_,u64>(0)?,r.get::<_,u64>(1)?,r.get::<_,u64>(2)?,r.get::<_,u64>(3)?)))?{let(time,count,rx,tx)=row?;let index=((time.saturating_sub(start))/bucket_seconds) as usize;if index<buckets{http_requests[index]+=count;rx_bytes[index]+=rx;tx_bytes[index]+=tx;}}
            let mut jobs=Vec::new();let mut q=tx.prepare("SELECT data FROM jobs")?;for row in q.query_map([],|r|r.get::<_,String>(0))?{let value:serde_json::Value=serde_json::from_str(&row?)?;let begin=value.get("started").and_then(|v|v.as_u64()).or_else(||value.get("created").and_then(|v|v.as_u64()));let finish=value.get("finished").and_then(|v|v.as_u64());if let Some(begin)=begin{jobs.push((begin,finish));}}
            let mut points=Vec::with_capacity(buckets);for i in 0..buckets{let time=start+bucket_seconds*i as u64;let sample_time=(time+bucket_seconds.saturating_sub(1)).min(now);let active_jobs=jobs.iter().filter(|(begin,finish)|*begin<=sample_time&&finish.map_or(true,|done|done>sample_time)).count();points.push(serde_json::json!({"time":time,"requests":requests[i],"successes":successes[i],"failures":failures[i],"active_jobs":active_jobs,"http_requests":http_requests[i],"rx_bytes":rx_bytes[i],"tx_bytes":tx_bytes[i]}));}
            Ok(serde_json::json!({"window_seconds":bucket_seconds*buckets as u64,"bucket_seconds":bucket_seconds,"generated_at":now,"totals":{"requests":requests.iter().sum::<u64>(),"successes":successes.iter().sum::<u64>(),"failures":failures.iter().sum::<u64>(),"http_requests":http_requests.iter().sum::<u64>(),"rx_bytes":rx_bytes.iter().sum::<u64>(),"tx_bytes":tx_bytes.iter().sum::<u64>()},"points":points}))
        })
    }
    pub fn prune_auth(&self) -> Result<()> { self.transaction(|tx| { tx.execute("DELETE FROM kv WHERE expires>0 AND expires<?1", [crate::util::now()])?; Ok(()) }) }
}
fn sensitive_operation_key(key:&str)->bool{let key=key.to_ascii_lowercase();["authorization","access_token","refresh_token","password","passwd","secret","api_key","apikey","owner_key","private_key","credential"].iter().any(|part|key.contains(part))}
fn sanitize_operation_value(value:&mut serde_json::Value){match value{serde_json::Value::Object(map)=>{for(key,value)in map.iter_mut(){if sensitive_operation_key(key){*value=serde_json::Value::String("[REDACTED]".into());}else if key.eq_ignore_ascii_case("script"){if let Some(text)=value.as_str(){*value=serde_json::json!({"omitted":true,"bytes":text.len(),"sha256":crate::util::digest(text)});}else{sanitize_operation_value(value);}}else{sanitize_operation_value(value);}}},serde_json::Value::Array(values)=>for value in values{sanitize_operation_value(value)},serde_json::Value::String(text)=>{for marker in ["Bearer ","token=","password=","secret=","api_key="]{if let Some(pos)=text.to_ascii_lowercase().find(&marker.to_ascii_lowercase()){let end=text[pos..].find(char::is_whitespace).map(|v|pos+v).unwrap_or(text.len());text.replace_range(pos..end,"[REDACTED]");break;}}},_=>{}}}
fn bounded_operation_text(text:&str,max:usize)->String{if text.len()<=max{return text.to_owned();}let mut end=max.min(text.len());while end>0&&!text.is_char_boundary(end){end-=1;}format!("{}\n… [truncated {} bytes]",&text[..end],text.len().saturating_sub(end))}
fn bounded_operation_json(mut value:serde_json::Value)->String{sanitize_operation_value(&mut value);let text=serde_json::to_string(&value).unwrap_or_else(|_|"{}".into());if text.len()<=262144{return text;}serde_json::json!({"truncated":true,"bytes":text.len(),"preview":bounded_operation_text(&text,131072)}).to_string()}
fn diff_stats(diff:&str)->(u64,u64){let mut added=0;let mut removed=0;for line in diff.lines(){if line.starts_with("+++")||line.starts_with("---"){continue;}if line.starts_with('+'){added+=1;}else if line.starts_with('-'){removed+=1;}}(added,removed)}

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
    #[test] fn dashboard_metrics_aggregate_audit_jobs_and_traffic(){let d=tempfile::tempdir().unwrap();let s=Store::open(&d.path().join("db")).unwrap();s.audit("read_file","demo/project","started","").unwrap();s.audit("read_file","demo/project","succeeded","").unwrap();s.record_traffic(1,120,340).unwrap();let now=crate::util::now();let job=serde_json::json!({"started":now.saturating_sub(1),"finished":null});s.transaction(|tx|{tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2)",rusqlite::params!["job",job.to_string()])?;Ok(())}).unwrap();let value=s.dashboard_metrics(3600,60).unwrap();assert_eq!(value["totals"]["requests"],1);assert_eq!(value["totals"]["successes"],1);assert_eq!(value["totals"]["failures"],0);assert_eq!(value["totals"]["http_requests"],1);assert_eq!(value["totals"]["rx_bytes"],120);assert_eq!(value["totals"]["tx_bytes"],340);assert_eq!(value["points"].as_array().unwrap().last().unwrap()["active_jobs"],1);}
    #[test] fn operation_log_redacts_and_tracks_diff(){let d=tempfile::tempdir().unwrap();let s=Store::open(&d.path().join("db")).unwrap();let id=s.operation_start("apply_patch","root","demo",&serde_json::json!({"authorization":"Bearer abc","path":"src/main.rs"})).unwrap();s.operation_finish(id,"succeeded",12,Some(&serde_json::json!({"changed":true})),"--- a/src/main.rs\n+++ b/src/main.rs\n-old\n+new\n","").unwrap();let detail=s.operation(id).unwrap();assert_eq!(detail["added_lines"],1);assert_eq!(detail["removed_lines"],1);assert_eq!(detail["input"]["authorization"],"[REDACTED]");}
}
