use crate::{store::Store,tools::types::{ListTaskCheckpointsArgs,TaskArgs},util};
use anyhow::{bail,Context,Result};
use rusqlite::params;
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};

const NAMESPACE:&str="task_checkpoints";
const MAX_RECORDS:i64=2000;
const MAX_JOBS:usize=20;

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct TaskJobRef{pub job_id:String,pub status:String,pub updated:u64}

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct TaskCheckpoint{
    pub task_id:String,
    pub workspace:String,
    pub project:String,
    pub stage:String,
    pub status:String,
    #[serde(default)]pub jobs:Vec<TaskJobRef>,
    #[serde(default)]pub last_commit:Option<String>,
    pub created:u64,
    pub updated:u64,
}

fn simple_id(kind:&str,value:&str)->Result<()>{
    if value.is_empty()||value.len()>128||!value.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_:. ".contains(&b)){bail!("{kind} must be 1..128 characters using letters, digits, spaces, '-', '_', ':' or '.'");}
    Ok(())
}
pub fn validate_task_id(task_id:&str)->Result<()>{simple_id("task_id",task_id)}
pub fn validate_context<'a>(task_id:&'a Option<String>,stage:&'a Option<String>)->Result<Option<(&'a str,&'a str)>>{
    match(task_id.as_deref(),stage.as_deref()){
        (None,None)=>Ok(None),
        (Some(task),Some(stage))=>{simple_id("task_id",task)?;simple_id("stage",stage)?;Ok(Some((task,stage)))}
        _=>bail!("task_id and stage must be provided together"),
    }
}
fn key(workspace:&str,project:&str,task_id:&str,stage:&str)->String{util::digest(format!("{workspace}\0{project}\0{task_id}\0{stage}"))}
fn load_stage(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str)->Result<Option<TaskCheckpoint>>{db.get(NAMESPACE,&key(workspace,project,task_id,stage))}
fn save(db:&Store,checkpoint:&TaskCheckpoint)->Result<()>{
    db.transaction(|tx|{
        crate::store::put(tx,NAMESPACE,&key(&checkpoint.workspace,&checkpoint.project,&checkpoint.task_id,&checkpoint.stage),checkpoint,0)?;
        tx.execute("DELETE FROM kv WHERE namespace=?1 AND key IN (SELECT key FROM kv WHERE namespace=?1 ORDER BY CAST(json_extract(value,'$.updated') AS INTEGER) DESC LIMIT -1 OFFSET ?2)",params![NAMESPACE,MAX_RECORDS])?;
        Ok(())
    })
}
fn base(workspace:&str,project:&str,task_id:&str,stage:&str)->TaskCheckpoint{let now=util::now();TaskCheckpoint{task_id:task_id.into(),workspace:workspace.into(),project:project.into(),stage:stage.into(),status:"started".into(),jobs:vec![],last_commit:None,created:now,updated:now}}
pub fn record_job(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str,job_id:&str,status:&str)->Result<()>{
    simple_id("task_id",task_id)?;simple_id("stage",stage)?;
    let mut c=load_stage(db,workspace,project,task_id,stage)?.unwrap_or_else(||base(workspace,project,task_id,stage));
    let now=util::now();if let Some(job)=c.jobs.iter_mut().find(|job|job.job_id==job_id){job.status=status.into();job.updated=now;}else{c.jobs.push(TaskJobRef{job_id:job_id.into(),status:status.into(),updated:now});if c.jobs.len()>MAX_JOBS{let drain=c.jobs.len()-MAX_JOBS;c.jobs.drain(..drain);}}
    c.status=format!("job_{status}");c.updated=now;save(db,&c)
}
pub fn record_commit(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str,commit:&str)->Result<()>{
    simple_id("task_id",task_id)?;simple_id("stage",stage)?;
    let mut c=load_stage(db,workspace,project,task_id,stage)?.unwrap_or_else(||base(workspace,project,task_id,stage));
    c.status="committed".into();c.last_commit=Some(commit.into());c.updated=util::now();save(db,&c)
}
fn query(db:&Store,workspace:Option<&str>,project:Option<&str>,task_id:Option<&str>,limit:usize)->Result<Vec<TaskCheckpoint>>{
    if limit==0||limit>200{bail!("limit must be 1..200");}
    if let Some(task)=task_id{simple_id("task_id",task)?;}
    db.transaction(|tx|{
        let mut q=tx.prepare("SELECT value FROM kv WHERE namespace=?1 AND (?2 IS NULL OR json_extract(value,'$.workspace')=?2) AND (?3 IS NULL OR json_extract(value,'$.project')=?3) AND (?4 IS NULL OR json_extract(value,'$.task_id')=?4) ORDER BY CAST(json_extract(value,'$.updated') AS INTEGER) DESC, rowid DESC LIMIT ?5")?;
        let rows=q.query_map(params![NAMESPACE,workspace,project,task_id,limit as i64],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(|text|Ok(serde_json::from_str(&text)?)).collect()
    })
}
pub fn get(db:&Store,a:TaskArgs)->Result<Value>{
    simple_id("task_id",&a.task_id)?;
    let stages=query(db,Some(&a.workspace),Some(&a.project),Some(&a.task_id),200)?;
    let latest=stages.first().cloned().context("Task checkpoint not found")?;
    Ok(json!({"task_id":a.task_id,"workspace":a.workspace,"project":a.project,"latest":latest,"stages":stages}))
}
pub fn list(db:&Store,a:ListTaskCheckpointsArgs)->Result<Value>{
    let checkpoints=query(db,a.workspace.as_deref(),a.project.as_deref(),a.task_id.as_deref(),a.limit)?;
    Ok(json!({"checkpoints":checkpoints}))
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn checkpoint_tracks_jobs_and_commit(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();record_job(&db,"root","demo","task-1","stage-a","job-1","queued").unwrap();record_job(&db,"root","demo","task-1","stage-a","job-1","succeeded").unwrap();record_commit(&db,"root","demo","task-1","stage-a","abc123").unwrap();let value=get(&db,TaskArgs{workspace:"root".into(),project:"demo".into(),task_id:"task-1".into()}).unwrap();assert_eq!(value["latest"]["status"],"committed");assert_eq!(value["latest"]["last_commit"],"abc123");assert_eq!(value["latest"]["jobs"][0]["status"],"succeeded");}
    #[test]fn task_and_stage_must_pair(){assert!(validate_context(&Some("task".into()),&None).is_err());assert!(validate_context(&None,&None).unwrap().is_none());}
}
