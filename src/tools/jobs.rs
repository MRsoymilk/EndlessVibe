use crate::{config::Config, runtime::Runtime, store::{self,Store}, tools::{process,tasks,types::*}, util};
use anyhow::{bail,Context,Result};
use rusqlite::{params,OptionalExtension};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use std::{collections::HashMap,fs::OpenOptions,io::Read,os::unix::fs::{MetadataExt,OpenOptionsExt},path::Path,sync::{Arc,Mutex},time::{Duration,Instant}};
use tokio::{io::{AsyncRead,AsyncReadExt},sync::{mpsc,Semaphore}};
use tokio_util::sync::CancellationToken;

#[derive(Clone,Serialize,Deserialize)] pub struct JobRecord { pub id:String,pub workspace:String,#[serde(default)]pub project:String,pub program:String,pub request_id:String,pub fingerprint:String,pub status:String,pub created:u64,#[serde(default)]pub created_ms:u64,pub started:Option<u64>,#[serde(default)]pub started_ms:Option<u64>,pub finished:Option<u64>,pub exit_code:Option<i32>,pub output_bytes_total:u64,pub output_truncated:bool,pub backend:String,pub error:Option<String>,#[serde(default)]pub preflight:Value,#[serde(default)]pub summary:Value,#[serde(default)]pub summary_capture:Value,#[serde(default)]pub task_id:Option<String>,#[serde(default)]pub stage:Option<String> }
#[derive(Serialize,Deserialize)] struct JobRef{id:String,fingerprint:String}
pub struct Jobs { db:Arc<Store>,config:Arc<Config>,slots:Arc<Semaphore>,submission:tokio::sync::Mutex<()>,active:Mutex<HashMap<String,CancellationToken>> }
fn explicit_preflight_programs(args:&CommandArgs)->Vec<String>{let mut programs=args.preflight_programs.clone();programs.sort();programs.dedup();programs}
fn merge_preflight_programs(mut explicit:Vec<String>,detected:&[String])->Vec<String>{explicit.extend(detected.iter().cloned());explicit.sort();explicit.dedup();explicit}
struct Output{bytes:Vec<u8>,offset:u64,total:u64}
const MAX_JOB_SUMMARY_BYTES:u64=1024*1024;
fn sensitive_summary_key(key:&str)->bool{let key=key.to_ascii_lowercase();["authorization","access_token","refresh_token","password","passwd","secret","api_key","apikey","owner_key","private_key","credential"].iter().any(|part|key.contains(part))}
fn sanitize_summary(value:&mut Value){match value{Value::Object(map)=>for(key,value)in map.iter_mut(){if sensitive_summary_key(key){*value=Value::String("[REDACTED]".into());}else{sanitize_summary(value);}},Value::Array(values)=>for value in values{sanitize_summary(value)},Value::String(text)=>{for marker in ["Bearer ","token=","password=","secret=","api_key="]{if let Some(pos)=text.to_ascii_lowercase().find(&marker.to_ascii_lowercase()){let end=text[pos..].find(char::is_whitespace).map(|v|pos+v).unwrap_or(text.len());text.replace_range(pos..end,"[REDACTED]");break;}}},_=>{}}}
fn capture_job_summary(path:&Path)->(Value,Value){
    let file=match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK).open(path){Ok(file)=>file,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return(Value::Null,json!({"status":"not_reported","reported":false,"cleanup_complete":true})),Err(error)=>return(Value::Null,json!({"status":"error","reported":true,"cleanup_complete":false,"error":util::bounded_text(&error.to_string(),512)}))};
    let result=(||->Result<(Value,usize)>{let md=file.metadata()?;if !md.is_file()||md.nlink()!=1||md.uid()!=unsafe{libc::geteuid()}||md.len()>MAX_JOB_SUMMARY_BYTES{bail!("unsafe or oversized job summary artifact");}let mut bytes=Vec::with_capacity(md.len() as usize);file.take(MAX_JOB_SUMMARY_BYTES+1).read_to_end(&mut bytes)?;if bytes.len() as u64>MAX_JOB_SUMMARY_BYTES{bail!("job summary exceeds 1 MiB");}let mut value:Value=serde_json::from_slice(&bytes).context("job summary is not valid JSON")?;if !value.is_object(){bail!("job summary must be a JSON object");}sanitize_summary(&mut value);Ok((value,bytes.len()))})();
    let cleanup_complete=std::fs::remove_file(path).is_ok();match result{Ok((value,bytes))=>(value,json!({"status":"captured","reported":true,"bytes":bytes,"cleanup_complete":cleanup_complete})),Err(error)=>(Value::Null,json!({"status":"error","reported":true,"cleanup_complete":cleanup_complete,"error":util::bounded_text(&error.to_string(),512)}))}
}
fn bubblewrap_failure_hint(backend:&str,status:&str,bytes:&[u8])->Option<String>{
    if backend!="bubblewrap"||status!="failed"{return None;}
    let text=String::from_utf8_lossy(bytes);let has_bwrap_stderr=|needle:&str|text.lines().any(|line|line.strip_prefix("[stderr] ").is_some_and(|line|line.starts_with("bwrap:")&&line.contains(needle)));
    if has_bwrap_stderr("Creating new namespace failed: Resource temporarily unavailable"){
        return Some("bubblewrap namespace creation failed with EAGAIN; check inherited RLIMIT_NPROC/cgroup pids limits and user-namespace availability, then rebuild/restart EndlessVibe after changing execution settings".into());
    }
    if has_bwrap_stderr("No permissions to create a new namespace"){
        return Some("bubblewrap cannot create an unprivileged user namespace; enable the kernel/distribution user-namespace setting or use another explicitly configured execution backend".into());
    }
    if has_bwrap_stderr("max_*_namespaces exceeded"){
        return Some("bubblewrap namespace quota is exhausted; inspect /proc/sys/user/max_*_namespaces and current namespace usage".into());
    }
    if has_bwrap_stderr("execvp ")&&has_bwrap_stderr("No such file or directory"){
        return Some("configured program is not visible inside the bubblewrap sandbox; add its sandbox path to execution.path and expose only the required toolchain directory with execution.readonly_mounts (do not mount the whole HOME or credential directories)".into());
    }
    None
}
impl Output{
    fn append(&mut self,stream:&str,chunk:&[u8],max:usize){let prefix=format!("[{stream}] ");self.bytes.extend_from_slice(prefix.as_bytes());self.bytes.extend_from_slice(chunk);self.total+=(prefix.len()+chunk.len()) as u64;let excess=self.bytes.len().saturating_sub(max);if excess>0{self.bytes.drain(..excess);self.offset+=excess as u64;}}
}
impl Jobs{
    pub fn new(db:Arc<Store>,config:Arc<Config>)->Result<Arc<Self>>{
        let s=Arc::new(Self{db,slots:Arc::new(Semaphore::new(config.limits.max_jobs)),config,submission:tokio::sync::Mutex::new(()),active:Mutex::new(HashMap::new())});
        let mut records=s.list_records(None,None,None,usize::MAX)?;
        for r in &mut records{if matches!(r.status.as_str(),"queued"|"running"){if let Ok(path)=process::job_summary_host_path(&s.config,&r.workspace,&r.project,&r.id){let(summary,capture)=capture_job_summary(&path);r.summary=summary;r.summary_capture=capture;}r.status="interrupted".into();r.finished=Some(util::now());r.error=Some("Service restarted; this command is never automatically replayed".into());s.save(r,None)?;if let (Some(task),Some(stage))=(r.task_id.as_deref(),r.stage.as_deref()){if let Err(error)=tasks::record_job(&s.db,&r.workspace,&r.project,task,stage,&r.id,"interrupted"){tracing::warn!(error=%error,job_id=%r.id,"Could not update task checkpoint after restart");}}}}
        Ok(s)
    }
    fn save(&self,r:&JobRecord,output:Option<&Output>)->Result<()>{self.db.transaction(|tx|{if let Some(out)=output{tx.execute("INSERT INTO jobs(id,data,output,output_offset) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET data=excluded.data,output=excluded.output,output_offset=excluded.output_offset",params![r.id,serde_json::to_string(r)?,out.bytes,out.offset])?;}else{tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",params![r.id,serde_json::to_string(r)?])?;}Ok(())})}
    fn load(&self,id:&str)->Result<JobRecord>{
        if id.len()>128{bail!("Invalid job ID");}
        let s:Option<String>=self.db.transaction(|tx|Ok(tx.query_row("SELECT data FROM jobs WHERE id=?1",[id],|r|r.get(0)).optional()?))?;
        Ok(serde_json::from_str(&s.context("Job not found or retention expired")?)?)
    }
    fn list_records(&self,workspace:Option<&str>,project:Option<&str>,task_id:Option<&str>,limit:usize)->Result<Vec<JobRecord>>{
        self.db.transaction(|tx|{let mut q=tx.prepare("SELECT data FROM jobs WHERE (?1 IS NULL OR json_extract(data,'$.workspace')=?1) AND (?2 IS NULL OR json_extract(data,'$.project')=?2) AND (?3 IS NULL OR json_extract(data,'$.task_id')=?3) ORDER BY json_extract(data,'$.created') DESC LIMIT ?4")?;let values=q.query_map(params![workspace,project,task_id,limit.min(10000) as i64],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;values.into_iter().map(|v|Ok(serde_json::from_str(&v)?)).collect()})
    }
    pub fn get(&self,id:&str)->Result<Value>{Ok(serde_json::to_value(self.load(id)?)?)}
    pub fn list(&self,a:ListJobsArgs)->Result<Value>{if a.limit==0||a.limit>200{bail!("limit must be 1..200");}if let Some(task)=&a.task_id{tasks::validate_task_id(task)?;}Ok(json!({"jobs":self.list_records(a.workspace.as_deref(),a.project.as_deref(),a.task_id.as_deref(),a.limit)?}))}
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
        tasks::validate_context(&a.task_id,&a.stage)?;
        let timeout=a.timeout_seconds.unwrap_or(rt.config.limits.command_timeout_seconds);if timeout==0||timeout>rt.config.limits.command_timeout_seconds{bail!("timeout_seconds exceeds the configured limit");}
        let _submission=self.submission.lock().await;
        let legacy_key=format!("{}:{}:{}",a.workspace,a.project,a.request_id);let legacy_fingerprint=util::digest(serde_json::to_vec(&(&a,shell))?);
        let w=rt.project(&a.workspace,&a.project)?;w.exec_allowed()?;let legacy_address=a.workspace!=w.workspace_id||a.project!=w.config.id;let mut a=a;a.workspace=w.workspace_id.clone();a.project=w.config.id.clone();let(effective_environment,effective_network,execution_profile,network_source)=process::effective_project_execution(&rt.config,&w,&a.environment,a.network)?;a.environment=effective_environment;a.network=Some(effective_network);
        let fingerprint=util::digest(serde_json::to_vec(&(&a,shell))?);let key=format!("{}:{}:{}",a.workspace,a.project,a.request_id);
        let existing=if let Some(existing)=self.db.get::<JobRef>("job_requests",&key)?{Some(existing)}else if legacy_key!=key{self.db.get::<JobRef>("job_requests",&legacy_key)?}else{None};
        if let Some(existing)=existing{
            if existing.fingerprint!=fingerprint&&existing.fingerprint!=legacy_fingerprint{return Err(crate::error::coded_details("IDEMPOTENCY_CONFLICT",false,"request_id was used with different command arguments",json!({"request_id":a.request_id})));}
            let mut value=self.get(&existing.id).unwrap_or_else(|_|json!({"id":existing.id.clone(),"status":"expired","message":"Output retention expired; request is not re-executed"}));value["job_id"]=json!(existing.id);value["reused"]=json!(true);value["canonical_workspace"]=json!(a.workspace);value["canonical_project"]=json!(a.project);return Ok(value);
        }
        let project_programs=process::detect_project_programs(&w);
        let detected_programs=process::detect_command_programs(&w,&a.cwd,&a.program,&a.args);let preflight_programs=merge_preflight_programs(explicit_preflight_programs(&a),&detected_programs);let mut preflight=process::preflight_job(&rt.config,&a.program,&preflight_programs,shell).await?;preflight["detected_programs"]=json!(detected_programs);preflight["execution_profile"]=json!(execution_profile);preflight["network"]=json!(effective_network);preflight["network_source"]=json!(network_source);preflight["environment_keys"]=json!(a.environment.keys().cloned().collect::<Vec<_>>());
        let job_id=util::random_secret()?;let summary_contract=process::job_summary_contract(&rt.config,&w.workspace_id,&w.config.id,&job_id)?;let command=process::build_job_command(&rt.config,&w,&a.program,&a.args,&a.cwd,shell,&project_programs,&a.environment,effective_network,&summary_contract.exposed_path)?;
        let permit=self.slots.clone().try_acquire_owned().map_err(|_|crate::error::coded("COMMAND_SLOTS_BUSY",true,"All command slots are occupied; query existing jobs first"))?;
        let lock=w.lock.clone().try_lock_owned().map_err(|_|crate::error::coded_details("PROJECT_BUSY",true,"another operation is using this project",json!({"workspace":a.workspace.clone(),"project":a.project.clone()})))?;
        let now=util::now();let record=JobRecord{id:job_id,workspace:a.workspace,project:a.project,program:if shell{"bash".into()}else{a.program},request_id:a.request_id,fingerprint:fingerprint.clone(),status:"queued".into(),created:now,created_ms:util::now_millis(),started:None,started_ms:None,finished:None,exit_code:None,output_bytes_total:0,output_truncated:false,backend:rt.config.execution.backend.clone(),error:None,preflight:preflight.clone(),summary:Value::Null,summary_capture:json!({"status":"pending","reported":false,"cleanup_complete":true}),task_id:a.task_id,stage:a.stage};
        let audit_target=format!("{}/{}",record.workspace,record.project);self.db.audit(if shell{"run_shell"}else{"run_command"},&audit_target,"accepted",&record.id)?;
        self.db.transaction(|tx|{tx.execute("INSERT INTO jobs(id,data) VALUES(?1,?2)",params![record.id,serde_json::to_string(&record)?])?;let expires=util::now()+7*86400;store::put(tx,"job_requests",&key,&JobRef{id:record.id.clone(),fingerprint:fingerprint.clone()},expires)?;if legacy_key!=key{store::put(tx,"job_requests",&legacy_key,&JobRef{id:record.id.clone(),fingerprint:fingerprint.clone()},expires)?;}Ok(())})?;
        if let (Some(task),Some(stage))=(record.task_id.as_deref(),record.stage.as_deref()){if let Err(error)=tasks::record_job(&self.db,&record.workspace,&record.project,task,stage,&record.id,"queued"){tracing::warn!(error=%error,job_id=%record.id,"Job accepted but task checkpoint persistence failed");}}
        let cancel=rt.shutdown.child_token();self.active.lock().map_err(|_|anyhow::anyhow!("Job table poisoned"))?.insert(record.id.clone(),cancel.clone());
        rt.publish_dashboard("job_changed",json!({"job_id":record.id,"workspace":record.workspace,"project":record.project,"program":record.program,"status":"queued"}));
        let result=json!({"job_id":record.id,"status":"queued","request_id":record.request_id,"preflight":preflight,"summary_contract":{"environment":"ENDLESSVIBE_JOB_SUMMARY","max_bytes":MAX_JOB_SUMMARY_BYTES},"task_id":record.task_id,"stage":record.stage,"reused":false,"canonical_workspace":record.workspace,"canonical_project":record.project,"legacy_addressing":legacy_address,"next":"get_job / get_job_output"});
        let this=self.clone();let event_rt=rt.clone();let summary_path=summary_contract.host_path;tokio::spawn(async move{let _permit=permit;let _lock=lock;let mut record=record;let result=this.worker(&mut record,command,cancel,timeout).await;if let Err(e)=result{record.status="failed".into();record.error=Some(util::bounded_text(&e.to_string(),1024));record.finished=Some(util::now());let _=this.save(&record,None);}let(summary,capture)=capture_job_summary(&summary_path);record.summary=summary;record.summary_capture=capture;let _=this.save(&record,None);if let (Some(task),Some(stage))=(record.task_id.as_deref(),record.stage.as_deref()){let _=tasks::record_job(&this.db,&record.workspace,&record.project,task,stage,&record.id,&record.status);}let audit_target=format!("{}/{}",record.workspace,record.project);let _=this.db.audit("job_finished",&audit_target,&record.status,&record.id);if let Ok(mut map)=this.active.lock(){map.remove(&record.id);}event_rt.publish_dashboard("job_changed",json!({"job_id":record.id,"workspace":record.workspace,"project":record.project,"program":record.program,"status":record.status,"exit_code":record.exit_code,"summary_reported":record.summary_capture["reported"],"task_id":record.task_id,"stage":record.stage}));let _=this.prune();});
        Ok(result)
    }
    fn prune(&self)->Result<()>{self.db.maintain(self.config.limits.retained_jobs).map(|_|())}
    async fn worker(&self,r:&mut JobRecord,mut cmd:tokio::process::Command,cancel:CancellationToken,timeout:u64)->Result<()>{
        if cancel.is_cancelled(){r.status="cancelled".into();r.finished=Some(util::now());return self.save(r,None);}
        let mut child=cmd.spawn().context("Could not start job (inspect executable, namespace support and sandbox mounts)")?;let pid=child.id().context("Missing child ID")?;let mut group=process::GroupGuard::new(pid);
        r.status="running".into();r.started=Some(util::now());r.started_ms=Some(util::now_millis());self.save(r,None)?;if let (Some(task),Some(stage))=(r.task_id.as_deref(),r.stage.as_deref()){if let Err(error)=tasks::record_job(&self.db,&r.workspace,&r.project,task,stage,&r.id,"running"){tracing::warn!(error=%error,job_id=%r.id,"Job is running but task checkpoint update failed");}}
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
        if r.status=="timed_out"{r.error=Some(format!("Execution exceeded {timeout} seconds; process group was terminated"));}else if let Some(hint)=bubblewrap_failure_hint(&r.backend,&r.status,&output.bytes){r.error=Some(hint);}self.save(r,Some(&output))
    }
}
async fn pipe<R:AsyncRead+Unpin>(mut r:R,label:&'static str,tx:mpsc::Sender<(&'static str,Vec<u8>)>){let mut b=[0u8;8192];loop{match r.read(&mut b).await{Ok(0)|Err(_)=>break,Ok(n)=>if tx.send((label,b[..n].to_vec())).await.is_err(){break;}}}}
#[cfg(test)]mod tests{use super::*;#[test]fn detected_command_programs_become_job_preflight(){let args=CommandArgs{workspace:"w".into(),project:"p".into(),program:"python3".into(),args:vec![],cwd:".".into(),request_id:"r".into(),timeout_seconds:None,preflight_programs:vec!["cargo".into(),"cargo".into()],environment:Default::default(),network:Some(false),task_id:None,stage:None};let merged=merge_preflight_programs(explicit_preflight_programs(&args),&["initdb".into(),"postgres".into(),"cargo".into()]);assert_eq!(merged,vec!["cargo".to_owned(),"initdb".to_owned(),"postgres".to_owned()]);}#[test]fn summary_capture_is_bounded_redacted_and_removed(){let d=tempfile::tempdir().unwrap();let path=d.path().join("summary.json");std::fs::write(&path,br#"{"passed":true,"password":"secret","nested":{"authorization":"Bearer abc"}}"#).unwrap();let(value,capture)=capture_job_summary(&path);assert_eq!(value["passed"],true);assert_eq!(value["password"],"[REDACTED]");assert_eq!(value["nested"]["authorization"],"[REDACTED]");assert_eq!(capture["status"],"captured");assert!(!path.exists());}#[test]fn ring_output_is_bounded(){let mut out=Output{bytes:vec![],offset:0,total:0};out.append("stdout",b"abcdefghijklmnopqrstuvwxyz",12);assert_eq!(out.bytes.len(),12);assert!(out.offset>0);assert_eq!(out.total,out.offset+out.bytes.len() as u64);}#[test]fn namespace_eagain_has_actionable_hint(){let h=bubblewrap_failure_hint("bubblewrap","failed",b"[stderr] bwrap: Creating new namespace failed: Resource temporarily unavailable\n").unwrap();assert!(h.contains("RLIMIT_NPROC"));}#[test]fn missing_sandbox_program_has_mount_hint(){let h=bubblewrap_failure_hint("bubblewrap","failed",b"[stderr] bwrap: execvp cargo: No such file or directory\n").unwrap();assert!(h.contains("readonly_mounts"));}#[test]fn source_text_cannot_fake_bwrap_hint(){assert!(bubblewrap_failure_hint("bubblewrap","failed",b"[stdout] + assert!(text.contains(\"bwrap: Creating new namespace failed: Resource temporarily unavailable\"));\n").is_none());}#[test]fn unrelated_failure_has_no_bwrap_hint(){assert!(bubblewrap_failure_hint("bubblewrap","failed",b"[stderr] cargo: error\n").is_none());assert!(bubblewrap_failure_hint("host","failed",b"[stderr] bwrap: Creating new namespace failed: Resource temporarily unavailable\n").is_none());}}
