use crate::{config::Config, runtime::Runtime, store::{self,Store}, tools::{process,types::*}, util};
use anyhow::{bail,Context,Result};
use rusqlite::{params,OptionalExtension};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{collections::HashMap,sync::{Arc,Mutex},time::{Duration,Instant}};
use tokio::{io::{AsyncRead,AsyncReadExt},sync::{mpsc,Semaphore}};
use tokio_util::sync::CancellationToken;

#[derive(Clone,Serialize,Deserialize)] pub struct JobRecord { pub id:String,pub workspace:String,pub program:String,pub request_id:String,pub fingerprint:String,pub status:String,pub created:u64,pub started:Option<u64>,pub finished:Option<u64>,pub exit_code:Option<i32>,pub output_bytes_total:u64,pub output_truncated:bool,pub backend:String,pub error:Option<String> }
#[derive(Serialize,Deserialize)] struct JobRef{id:String,fingerprint:String}
pub struct Jobs { db:Arc<Store>,config:Arc<Config>,slots:Arc<Semaphore>,submission:tokio::sync::Mutex<()>,active:Mutex<HashMap<String,CancellationToken>> }
struct Output{bytes:Vec<u8>,offset:u64,total:u64}
impl Output{
    fn append(&mut self,stream:&str,chunk:&[u8],max:usize){let prefix=format!("[{stream}] ");self.bytes.extend_from_slice(prefix.as_bytes());self.bytes.extend_from_slice(chunk);self.total+=(prefix.len()+chunk.len()) as u64;let excess=self.bytes.len().saturating_sub(max);if excess>0{self.bytes.drain(..excess);self.offset+=excess as u64;}}
}
impl Jobs{
    pub fn new(db:Arc<Store>,config:Arc<Config>)->Result<Arc<Self>>{
        let s=Arc::new(Self{db,slots:Arc::new(Semaphore::new(config.limits.max_jobs)),config,submission:tokio::sync::Mutex::new(()),active:Mutex::new(HashMap::new())});
        let mut records=s.list_records(None,usize::MAX)?;
        for r in &mut records{if matches!(r.status.as_str(),"queued"|"running"){r.status="interrupted".into();r.finished=Some(util::now());r.error=Some("Service restarted; this command is never automatically replayed".into());s.save(r,None)?;}}
        Ok(s)
    }
    fn save(&self,r:&JobRecord,output:Option<&Output>)->Result<()>{self.db.transaction(|tx|{if let Some(out)=output{tx.execute("INSERT INTO jobs(id,data,output,output_offset) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET data=excluded.data,output=excluded.output,output_offset=excluded.output_offset",params![r.id,serde_json::to_string(r)?,out.bytes,out.offset])?;}else{tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![r.id,serde_json::to_string(r)?])?;}Ok(())})}
    fn load(&self,id:&str)->Result<JobRecord>{
        if id.len()>128{bail!("Invalid job ID");}
        let s:Option<String>=self.db.transaction(|tx|Ok(tx.query_row("SELECT data FROM jobs WHERE id=?1",[id],|r|r.get(0)).optional()?))?;
        Ok(serde_json::from_str(&s.context("Job not found or retention expired")?)?)
    }
    fn list_records(&self,workspace:Option<&str>,limit:usize)->Result<Vec<JobRecord>>{
        self.db.transaction(|tx|{let mut q=tx.prepare("SELECT data FROM jobs WHERE (?1 IS NULL OR json_extract(data,'$.workspace')=?1) ORDER BY json_extract(data,'$.created') DESC LIMIT ?2")?;let values=q.query_map(params![workspace,limit.min(10000) as i64],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;values.into_iter().map(|v|Ok(serde_json::from_str(&v)?)).collect()})
    }
    pub fn get(&self,id:&str)->Result<Value>{Ok(serde_json::to_value(self.load(id)?)?)}
    pub fn list(&self,a:ListJobsArgs)->Result<Value>{if a.limit==0||a.limit>200{bail!("limit must be 1..200");}Ok(json!({"jobs":self.list_records(a.workspace.as_deref(),a.limit)?}))}
    pub fn output(&self,a:OutputArgs)->Result<Value>{
        if a.limit==0||a.limit>262144{bail!("limit must be 1..262144");}
        let record=self.load(&a.job_id)?;
        let (bytes,base):(Vec<u8>,u64)=self.db.transaction(|tx|Ok(tx.query_row("SELECT output,output_offset FROM jobs WHERE id=?1",[&a.job_id],|r|Ok((r.get(0)?,r.get(1)?)))?))?;
        let start=a.offset.max(base).min(base+bytes.len() as u64);let from=(start-base) as usize;let end=from.saturating_add(a.limit).min(bytes.len());
        Ok(json!({"job_id":a.job_id,"status":record.status,"output":String::from_utf8_lossy(&bytes[from..end]),"offset":start,"next_offset":base+end as u64,"dropped_before":base,"requested_offset_was_dropped":a.offset<base,"has_more":end<bytes.len(),"cursor_unit":"raw UTF-8 bytes; split characters may display as replacement characters"}))
    }
    pub fn cancel(&self,id:&str)->Result<Value>{let record=self.load(id)?;let map=self.active.lock().map_err(|_|anyhow::anyhow!("Job table poisoned"))?;if let Some(c)=map.get(id){c.cancel();Ok(json!({"job_id":id,"cancellation_requested":true}))}else{Ok(json!({"job_id":id,"cancellation_requested":false,"status":record.status}))}}
    pub fn cancel_all(&self){if let Ok(map)=self.active.lock(){for token in map.values(){token.cancel();}}}
    pub fn active_count(&self)->usize{self.active.lock().map(|m|m.len()).unwrap_or(0)}
    pub async fn shutdown(&self){self.cancel_all();let until=Instant::now()+Duration::from_secs(5);while self.active_count()>0&&Instant::now()<until{tokio::time::sleep(Duration::from_millis(50)).await;}}
    pub async fn submit(self:&Arc<Self>,rt:Arc<Runtime>,a:CommandArgs,shell:bool)->Result<Value>{
        if a.request_id.is_empty()||a.request_id.len()>128||!a.request_id.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_:.".contains(&b)){bail!("Provide a unique simple request_id (1..128 characters); retries with the same ID never rerun the job during retention");}
        let timeout=a.timeout_seconds.unwrap_or(rt.config.limits.command_timeout_seconds);if timeout==0||timeout>rt.config.limits.command_timeout_seconds{bail!("timeout_seconds exceeds the configured limit");}
        let _submission=self.submission.lock().await;
        let fingerprint=util::digest(serde_json::to_vec(&(&a,shell))?);let key=format!("{}:{}",a.workspace,a.request_id);
        if let Some(existing)=self.db.get::<JobRef>("job_requests",&key)?{
            if existing.fingerprint!=fingerprint{bail!("IDEMPOTENCY_CONFLICT: request_id was used with different command arguments");}
            let mut value=self.get(&existing.id).unwrap_or_else(|_|json!({"id":existing.id.clone(),"status":"expired","message":"Output retention expired; request is not re-executed"}));value["job_id"]=json!(existing.id);value["reused"]=json!(true);return Ok(value);
        }
        let w=rt.workspace(&a.workspace)?;w.exec_allowed()?;
        let permit=self.slots.clone().try_acquire_owned().context("All command slots are occupied; query existing jobs first")?;
        let lock=w.lock.clone().try_lock_owned().context("WORKSPACE_BUSY: another operation is using this workspace")?;
        let command=process::build_job_command(&rt.config,&w,&a.program,&a.args,&a.cwd,shell)?;
        let record=JobRecord{id:util::random_secret()?,workspace:a.workspace,program:if shell{"bash".into()}else{a.program},request_id:a.request_id,fingerprint:fingerprint.clone(),status:"queued".into(),created:util::now(),started:None,finished:None,exit_code:None,output_bytes_total:0,output_truncated:false,backend:rt.config.execution.backend.clone(),error:None};
        self.db.audit(if shell{"run_shell"}else{"run_command"},&record.workspace,"accepted",&record.id)?;
        self.db.transaction(|tx|{tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2)",params![record.id,serde_json::to_string(&record)?])?;store::put(tx,"job_requests",&key,&JobRef{id:record.id.clone(),fingerprint},util::now()+7*86400)?;Ok(())})?;
        let cancel=rt.shutdown.child_token();self.active.lock().map_err(|_|anyhow::anyhow!("Job table poisoned"))?.insert(record.id.clone(),cancel.clone());
        let result=json!({"job_id":record.id,"status":"queued","request_id":record.request_id,"reused":false,"next":"get_job / get_job_output"});
        let this=self.clone();tokio::spawn(async move{let _permit=permit;let _lock=lock;let mut record=record;let result=this.worker(&mut record,command,cancel,timeout).await;if let Err(e)=result{record.status="failed".into();record.error=Some(util::bounded_text(&e.to_string(),1024));record.finished=Some(util::now());let _=this.save(&record,None);}let _=this.db.audit("job_finished",&record.workspace,&record.status,&record.id);if let Ok(mut map)=this.active.lock(){map.remove(&record.id);}let _=this.prune();});
        Ok(result)
    }
    fn prune(&self)->Result<()>{self.db.transaction(|tx|{tx.execute("DELETE FROM jobs WHERE id IN (SELECT id FROM jobs WHERE json_extract(data,'$.status') NOT IN ('queued','running') ORDER BY json_extract(data,'$.created') DESC LIMIT -1 OFFSET ?1)",[self.config.limits.retained_jobs as i64])?;Ok(())})}
    async fn worker(&self,r:&mut JobRecord,mut cmd:tokio::process::Command,cancel:CancellationToken,timeout:u64)->Result<()>{
        if cancel.is_cancelled(){r.status="cancelled".into();r.finished=Some(util::now());return self.save(r,None);}
        let mut child=cmd.spawn().context("Could not start job (inspect executable, namespace support and sandbox mounts)")?;let pid=child.id().context("Missing child ID")?;let mut group=process::GroupGuard::new(pid);
        r.status="running".into();r.started=Some(util::now());self.save(r,None)?;
        let (tx,mut rx)=mpsc::channel::<(&'static str,Vec<u8>)>(16);
        let out=tokio::spawn(pipe(child.stdout.take().context("stdout missing")?,"stdout",tx.clone()));let err=tokio::spawn(pipe(child.stderr.take().context("stderr missing")?,"stderr",tx.clone()));drop(tx);
        let mut output=Output{bytes:vec![],offset:0,total:0};let mut last_save=Instant::now();let deadline=tokio::time::sleep(Duration::from_secs(timeout));tokio::pin!(deadline);
        let wait=child.wait();tokio::pin!(wait);let mut forced:Option<&str>=None;let mut pipes_open=true;
        let status=loop{tokio::select!{
            status=&mut wait=>break status?,
            chunk=rx.recv(),if pipes_open=>{if let Some((stream,bytes))=chunk{output.append(stream,&bytes,self.config.limits.max_output_bytes);if last_save.elapsed()>=Duration::from_millis(250){r.output_bytes_total=output.total;r.output_truncated=output.offset>0;self.save(r,Some(&output))?;last_save=Instant::now();}}else{pipes_open=false;}},
            _=cancel.cancelled()=>{forced=Some("cancelled");process::terminate_group(pid).await;break wait.await?;},
            _=&mut deadline=>{forced=Some("timed_out");process::terminate_group(pid).await;break wait.await?;}
        }};
        group.kill();
        let _=tokio::time::timeout(Duration::from_secs(2),async{while let Some((stream,bytes))=rx.recv().await{output.append(stream,&bytes,self.config.limits.max_output_bytes);}}).await;
        out.abort();err.abort();r.exit_code=status.code();r.status=forced.unwrap_or(if status.success(){"succeeded"}else{"failed"}).into();r.finished=Some(util::now());r.output_bytes_total=output.total;r.output_truncated=output.offset>0;
        if r.status=="timed_out"{r.error=Some(format!("Execution exceeded {timeout} seconds; process group was terminated"));}self.save(r,Some(&output))
    }
}
async fn pipe<R:AsyncRead+Unpin>(mut r:R,label:&'static str,tx:mpsc::Sender<(&'static str,Vec<u8>)>){let mut b=[0u8;8192];loop{match r.read(&mut b).await{Ok(0)|Err(_)=>break,Ok(n)=>if tx.send((label,b[..n].to_vec())).await.is_err(){break;}}}}
#[cfg(test)]mod tests{use super::*;#[test]fn ring_output_is_bounded(){let mut out=Output{bytes:vec![],offset:0,total:0};out.append("stdout",b"abcdefghijklmnopqrstuvwxyz",12);assert_eq!(out.bytes.len(),12);assert!(out.offset>0);assert_eq!(out.total,out.offset+out.bytes.len() as u64);}}
