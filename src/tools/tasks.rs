use crate::{store::Store,task::TaskState,tools::types::{ListTaskCheckpointsArgs,TaskArgs},util};
use anyhow::{bail,Context,Result};
use rusqlite::{params,OptionalExtension};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};

const NAMESPACE:&str="task_checkpoints";
const MAX_JOBS:usize=20;
const MAX_OPERATIONS:usize=40;
const AUTO_JOB_PREFIX:&str="auto-job-";
pub fn automatic_job_context(job_id:&str)->(String,String){(format!("{AUTO_JOB_PREFIX}{job_id}"),"execute".into())}

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct TaskJobRef{pub job_id:String,pub status:String,pub updated:u64}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct TaskOperationRef{pub operation_id:i64,pub tool:String,pub status:String,pub updated:u64}

#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct TaskCheckpoint{
    pub task_id:String,
    pub workspace:String,
    pub project:String,
    pub stage:String,
    pub status:String,
    #[serde(default)]pub origin:String,
    #[serde(default)]pub jobs:Vec<TaskJobRef>,
    #[serde(default)]pub operations:Vec<TaskOperationRef>,
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
fn base(workspace:&str,project:&str,task_id:&str,stage:&str)->TaskCheckpoint{let now=util::now();TaskCheckpoint{task_id:task_id.into(),workspace:workspace.into(),project:project.into(),stage:stage.into(),status:TaskState::Pending.to_string(),origin:if task_id.starts_with(AUTO_JOB_PREFIX){"auto_job"}else{"explicit"}.into(),jobs:vec![],operations:vec![],last_commit:None,created:now,updated:now}}
fn modify_stage(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str,mutate:impl FnOnce(&mut TaskCheckpoint))->Result<()> {let stage_key=key(workspace,project,task_id,stage);db.transaction(|tx|{let mut c=crate::store::get::<TaskCheckpoint>(tx,NAMESPACE,&stage_key)?.unwrap_or_else(||base(workspace,project,task_id,stage));mutate(&mut c);crate::store::put(tx,NAMESPACE,&stage_key,&c,0)?;crate::store::prune_common(tx,util::now())?;Ok(())})}
pub fn start_task(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str)->Result<Value>{simple_id("task_id",task_id)?;simple_id("stage",stage)?;if task_id.starts_with(AUTO_JOB_PREFIX){bail!("auto-job- prefix is reserved for standalone Jobs");}let stage_key=key(workspace,project,task_id,stage);let(checkpoint,created)=db.transaction(|tx|{let existing=crate::store::get::<TaskCheckpoint>(tx,NAMESPACE,&stage_key)?;let created=existing.is_none();let checkpoint=existing.unwrap_or_else(||base(workspace,project,task_id,stage));if created{crate::store::put(tx,NAMESPACE,&stage_key,&checkpoint,0)?;crate::store::prune_common(tx,util::now())?;}Ok((checkpoint,created))})?;Ok(json!({"workspace":workspace,"project":project,"task_id":task_id,"stage":stage,"status":checkpoint.status,"created":created,"next":"Pass this task_id + stage in subsequent MCP operations. A durable stage requires git_commit with the same context."}))}
pub fn record_operation(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str,operation_id:i64,tool:&str,status:&str)->Result<()>{simple_id("task_id",task_id)?;simple_id("stage",stage)?;if operation_id<=0{bail!("Invalid operation ID");}modify_stage(db,workspace,project,task_id,stage,|c|{let now=util::now();if let Some(item)=c.operations.iter_mut().find(|op|op.operation_id==operation_id){item.status=status.into();item.updated=now;}else{c.operations.push(TaskOperationRef{operation_id,tool:util::bounded_text(tool,64),status:status.into(),updated:now});if c.operations.len()>MAX_OPERATIONS{let excess=c.operations.len()-MAX_OPERATIONS;c.operations.drain(..excess);}}c.updated=now;})}
pub fn backfill_legacy_job(db:&Store,workspace:&str,project:&str,job_id:&str,status:&str,created:u64,updated:u64)->Result<()>{let(task_id,stage)=automatic_job_context(job_id);let stage_key=key(workspace,project,&task_id,&stage);db.transaction(|tx|{if crate::store::get::<TaskCheckpoint>(tx,NAMESPACE,&stage_key)?.is_some(){return Ok(());}let mut c=base(workspace,project,&task_id,&stage);c.created=created;c.updated=updated;c.status=TaskState::from_job(status).unwrap_or(TaskState::Failed).to_string();c.jobs.push(TaskJobRef{job_id:job_id.to_owned(),status:status.into(),updated});crate::store::put(tx,NAMESPACE,&stage_key,&c,0)?;crate::store::prune_common(tx,util::now())?;Ok(())})}
pub fn record_job(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str,job_id:&str,status:&str)->Result<()>{
    simple_id("task_id",task_id)?;simple_id("stage",stage)?;
    modify_stage(db,workspace,project,task_id,stage,|c|{let now=util::now();if let Some(job)=c.jobs.iter_mut().find(|job|job.job_id==job_id){job.status=status.into();job.updated=now;}else{c.jobs.push(TaskJobRef{job_id:job_id.into(),status:status.into(),updated:now});if c.jobs.len()>MAX_JOBS{let drain=c.jobs.len()-MAX_JOBS;c.jobs.drain(..drain);}}if c.last_commit.is_none(){c.status=TaskState::from_job(status).unwrap_or(TaskState::Failed).to_string();}c.updated=now;})
}
pub fn record_commit(db:&Store,workspace:&str,project:&str,task_id:&str,stage:&str,commit:&str)->Result<()>{
    simple_id("task_id",task_id)?;simple_id("stage",stage)?;
    modify_stage(db,workspace,project,task_id,stage,|c|{c.status=TaskState::Committed.to_string();c.last_commit=Some(commit.into());c.updated=util::now();})
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
pub fn dashboard_list(db:&Store,explicit_limit:usize,auto_limit:usize)->Result<Value>{
    if explicit_limit==0||explicit_limit>200||auto_limit>100{bail!("explicit_limit must be 1..200 and auto_limit 0..100");}
    db.transaction(|tx|{
        let explicit_filter="COALESCE(json_extract(value,'$.origin'),'explicit')!='auto_job'";
        let auto_filter="json_extract(value,'$.origin')='auto_job'";
        let scan=|filter:&str,limit:usize|->Result<Vec<TaskCheckpoint>>{
            let sql=format!("SELECT value FROM kv WHERE namespace=?1 AND {filter} ORDER BY CAST(json_extract(value,'$.updated') AS INTEGER) DESC,rowid DESC LIMIT ?2");
            let mut query=tx.prepare(&sql)?;
            let rows=query.query_map(params![NAMESPACE,limit as i64],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            rows.into_iter().map(|row|Ok(serde_json::from_str(&row)?)).collect()
        };
        let explicit=scan(explicit_filter,explicit_limit)?;let auto=scan(auto_filter,auto_limit)?;
        let total_explicit_stages:i64=tx.query_row(&format!("SELECT COUNT(*) FROM kv WHERE namespace=?1 AND {explicit_filter}"),[NAMESPACE],|r|r.get(0))?;
        let total_auto_jobs:i64=tx.query_row(&format!("SELECT COUNT(*) FROM kv WHERE namespace=?1 AND {auto_filter}"),[NAMESPACE],|r|r.get(0))?;
        let total_explicit_tasks:i64=tx.query_row(&format!("SELECT COUNT(*) FROM (SELECT json_extract(value,'$.workspace'),json_extract(value,'$.project'),json_extract(value,'$.task_id') FROM kv WHERE namespace=?1 AND {explicit_filter} GROUP BY 1,2,3)"),[NAMESPACE],|r|r.get(0))?;
        let visible_explicit_stages=explicit.len();let visible_auto_jobs=auto.len();let mut checkpoints=explicit.into_iter().map(|c|serde_json::to_value(c)).collect::<serde_json::Result<Vec<_>>>()?;
        for cp in auto{let mut value=serde_json::to_value(&cp)?;if let Some(job_id)=cp.jobs.first().map(|job|job.job_id.as_str()){
            let detail:Option<(Option<String>,Option<String>)>=tx.query_row("SELECT json_extract(data,'$.program'),json_extract(data,'$.request_id') FROM jobs WHERE id=?1",[job_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((program,request_id))=detail{value["program"]=json!(program);value["request_id"]=json!(request_id);}
        }checkpoints.push(value);}
        Ok(json!({"checkpoints":checkpoints,"total_explicit_tasks":total_explicit_tasks,"total_explicit_stages":total_explicit_stages,"total_auto_jobs":total_auto_jobs,"visible_explicit_stages":visible_explicit_stages,"visible_auto_jobs":visible_auto_jobs,"explicit_truncated":(visible_explicit_stages as i64)<total_explicit_stages,"auto_truncated":(visible_auto_jobs as i64)<total_auto_jobs}))
    })
}
pub fn get(db:&Store,a:TaskArgs)->Result<Value>{
    simple_id("task_id",&a.task_id)?;
    let stages=query(db,Some(&a.workspace),Some(&a.project),Some(&a.task_id),50)?;
    let latest=stages.first().cloned().context("Task checkpoint not found")?;
    Ok(json!({"task_id":a.task_id,"workspace":a.workspace,"project":a.project,"latest":latest,"stages":stages,"history_limit":50}))
}
pub fn recovery(db:&Store,a:TaskArgs)->Result<Value>{
    simple_id("task_id",&a.task_id)?;let stages=query(db,Some(&a.workspace),Some(&a.project),Some(&a.task_id),200)?;let latest=stages.first().cloned().context("Task checkpoint not found")?;let latest_job=latest.jobs.iter().max_by_key(|job|job.updated).cloned();let last_checkpoint=stages.iter().filter(|stage|stage.status==TaskState::Committed.to_string()).max_by_key(|stage|stage.updated).and_then(|stage|stage.last_commit.as_ref().map(|commit|json!({"stage":stage.stage,"commit":commit,"updated":stage.updated})));let checkpoint_count=stages.iter().filter(|stage|stage.status==TaskState::Committed.to_string()).count();let(kind,message)=if latest.origin=="auto_job"&&latest.status=="succeeded"{("job_complete","This one-off Job succeeded. No Git checkpoint was created; use an explicit multi-stage Task to group a longer workflow.")}else{match latest.status.as_str(){"pending"|"running"=>("poll_job","A Job for the current stage may still be active. Poll the latest Job and do not start a duplicate command."),"succeeded"=>("checkpoint_stage","The latest Job passed but this stage is not durable yet. Review git_status and all git_diff pages, then commit with the same task_id + stage."),"committed"=>("start_next_stage","The current stage is already durable. Begin the next logical stage with the same task_id and a new stage name."),"failed"|"cancelled"|"interrupted"=>("inspect_and_retry","Inspect the latest Job and fetch only the log pages needed for diagnosis. After fixing the cause, retry the same stage with a new request_id."),_=>("inspect_task","Inspect the current stage before continuing.")}};Ok(json!({"task_id":a.task_id,"workspace":a.workspace,"project":a.project,"recoverable":true,"current_stage":latest.stage,"status":latest.status,"updated":latest.updated,"latest_job":latest_job,"last_checkpoint":last_checkpoint,"stage_count":stages.len(),"checkpoint_count":checkpoint_count,"recommended_action":{"kind":kind,"stage":latest.stage,"job_id":latest_job.as_ref().map(|job|job.job_id.clone()),"message":message}}))
}
pub fn continue_recovery(db:&Store,a:crate::tools::types::ContinueTaskArgs)->Result<Value>{let(task_id,selection)=if let Some(task_id)=a.task_id{simple_id("task_id",&task_id)?;(task_id,"explicit")}else{let preferred:Option<TaskCheckpoint>=db.transaction(|tx|{let text:Option<String>=tx.query_row("SELECT value FROM kv WHERE namespace=?1 AND json_extract(value,'$.workspace')=?2 AND json_extract(value,'$.project')=?3 AND COALESCE(json_extract(value,'$.origin'),'explicit')!='auto_job' ORDER BY CAST(json_extract(value,'$.updated') AS INTEGER) DESC,rowid DESC LIMIT 1",params![NAMESPACE,a.workspace,a.project],|r|r.get(0)).optional()?;text.map(|s|Ok(serde_json::from_str(&s)?)).transpose()})?;if let Some(cp)=preferred{(cp.task_id,"latest_explicit_task")}else{let latest=query(db,Some(&a.workspace),Some(&a.project),None,1)?.into_iter().next().context("No task checkpoint found for this project")?;(latest.task_id,"latest_project_job")}};let mut value=recovery(db,TaskArgs{workspace:a.workspace,project:a.project,task_id:task_id.clone()})?;value["selection"]=json!(selection);value["resolved_task_id"]=json!(task_id);Ok(value)}
pub fn list(db:&Store,a:ListTaskCheckpointsArgs)->Result<Value>{
    let checkpoints=query(db,a.workspace.as_deref(),a.project.as_deref(),a.task_id.as_deref(),a.limit)?;
    Ok(json!({"checkpoints":checkpoints}))
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn checkpoint_tracks_jobs_and_commit(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();record_job(&db,"root","demo","task-1","stage-a","job-1","queued").unwrap();record_job(&db,"root","demo","task-1","stage-a","job-1","succeeded").unwrap();record_commit(&db,"root","demo","task-1","stage-a","abc123").unwrap();let value=get(&db,TaskArgs{workspace:"root".into(),project:"demo".into(),task_id:"task-1".into()}).unwrap();assert_eq!(value["latest"]["status"],"committed");assert_eq!(value["latest"]["last_commit"],"abc123");assert_eq!(value["latest"]["jobs"][0]["status"],"succeeded");}
    #[test]fn legacy_backfill_preserves_times_and_is_idempotent(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();backfill_legacy_job(&db,"w","p","old-id","failed",123,456).unwrap();backfill_legacy_job(&db,"w","p","old-id","running",999,999).unwrap();let(t,_)=automatic_job_context("old-id");let cp=get(&db,TaskArgs{workspace:"w".into(),project:"p".into(),task_id:t}).unwrap();assert_eq!(cp["latest"]["created"],123);assert_eq!(cp["latest"]["updated"],456);assert_eq!(cp["latest"]["status"],"failed");}
    #[test]fn auto_job_context_is_unique_and_is_not_committed(){let(a,s)=automatic_job_context("job-abc");let(b,_)=automatic_job_context("job-xyz");assert_ne!(a,b);assert_eq!(s,"execute");let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();record_job(&db,"root","app",&a,&s,"job-abc","succeeded").unwrap();let result=get(&db,TaskArgs{workspace:"root".into(),project:"app".into(),task_id:a}).unwrap();assert_eq!(result["latest"]["origin"],"auto_job");assert_eq!(result["latest"]["status"],"succeeded");assert!(result["latest"]["last_commit"].is_null());let recovered=recovery(&db,TaskArgs{workspace:"root".into(),project:"app".into(),task_id:result["task_id"].as_str().unwrap().into()}).unwrap();assert_eq!(recovered["recommended_action"]["kind"],"job_complete");}
    #[test]fn start_and_link_operations_keeps_stage_status(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();assert_eq!(start_task(&db,"w","p","feature-x","edit").unwrap()["created"],true);assert_eq!(start_task(&db,"w","p","feature-x","edit").unwrap()["created"],false);record_operation(&db,"w","p","feature-x","edit",41,"apply_patch","succeeded").unwrap();let v=get(&db,TaskArgs{workspace:"w".into(),project:"p".into(),task_id:"feature-x".into()}).unwrap();assert_eq!(v["latest"]["operations"][0]["operation_id"],41);assert_eq!(v["latest"]["status"],"pending");record_commit(&db,"w","p","feature-x","edit","abc123").unwrap();record_operation(&db,"w","p","feature-x","edit",42,"git_commit","succeeded").unwrap();let v=get(&db,TaskArgs{workspace:"w".into(),project:"p".into(),task_id:"feature-x".into()}).unwrap();assert_eq!(v["latest"]["status"],"committed");}
    #[test]fn auto_job_does_not_steal_explicit_task_resume(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();start_task(&db,"w","p","actual-project-work","inspect").unwrap();let(auto,stage)=automatic_job_context("random-job");record_job(&db,"w","p",&auto,&stage,"random-job","succeeded").unwrap();let result=continue_recovery(&db,crate::tools::types::ContinueTaskArgs{workspace:"w".into(),project:"p".into(),task_id:None}).unwrap();assert_eq!(result["resolved_task_id"],"actual-project-work");assert_eq!(result["selection"],"latest_explicit_task");}
    #[test]fn committed_stage_cannot_be_downgraded_by_late_job(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();record_job(&db,"w","p","task-1","build","job-1","succeeded").unwrap();record_commit(&db,"w","p","task-1","build","abc123").unwrap();record_job(&db,"w","p","task-1","build","job-2","running").unwrap();record_job(&db,"w","p","task-1","build","job-2","failed").unwrap();let cp=get(&db,TaskArgs{workspace:"w".into(),project:"p".into(),task_id:"task-1".into()}).unwrap();assert_eq!(cp["latest"]["status"],"committed");assert_eq!(cp["latest"]["last_commit"],"abc123");assert_eq!(cp["latest"]["jobs"][1]["status"],"failed");}
    #[test]fn concurrent_operations_are_not_lost(){let d=tempfile::tempdir().unwrap();let db=std::sync::Arc::new(Store::open(&d.path().join("db")).unwrap());start_task(&db,"w","p","concurrent","edit").unwrap();let handles=(0..30).map(|i|{let db=db.clone();std::thread::spawn(move||record_operation(&db,"w","p","concurrent","edit",i+1,"read_file","succeeded").unwrap())}).collect::<Vec<_>>();for h in handles{h.join().unwrap();}let cp=get(&db,TaskArgs{workspace:"w".into(),project:"p".into(),task_id:"concurrent".into()}).unwrap();assert_eq!(cp["latest"]["operations"].as_array().unwrap().len(),30);}
    #[test]fn dashboard_keeps_real_tasks_visible_after_auto_job_backlog(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();start_task(&db,"root","app","meaningful-task","inspect").unwrap();for i in 0..120{backfill_legacy_job(&db,"root","app",&format!("job-{i}"),"succeeded",1,2).unwrap();}let value=dashboard_list(&db,20,5).unwrap();assert_eq!(value["total_explicit_tasks"],1);assert_eq!(value["total_explicit_stages"],1);assert_eq!(value["total_auto_jobs"],120);assert_eq!(value["checkpoints"].as_array().unwrap().len(),6);assert_eq!(value["checkpoints"][0]["task_id"],"meaningful-task");assert_eq!(value["visible_auto_jobs"],5);assert_eq!(value["auto_truncated"],true);}
    #[test]fn task_and_stage_must_pair(){assert!(validate_context(&Some("task".into()),&None).is_err());assert!(validate_context(&None,&None).unwrap().is_none());}
    #[test]fn recovery_points_to_checkpoint_after_passed_job(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();record_job(&db,"root","demo","task-1","prepare","job-1","succeeded").unwrap();record_commit(&db,"root","demo","task-1","prepare","abc123").unwrap();record_job(&db,"root","demo","task-1","validate","job-2","succeeded").unwrap();let value=recovery(&db,TaskArgs{workspace:"root".into(),project:"demo".into(),task_id:"task-1".into()}).unwrap();assert_eq!(value["current_stage"],"validate");assert_eq!(value["latest_job"]["job_id"],"job-2");assert_eq!(value["last_checkpoint"]["stage"],"prepare");assert_eq!(value["last_checkpoint"]["commit"],"abc123");assert_eq!(value["recommended_action"]["kind"],"checkpoint_stage");record_job(&db,"root","demo","task-1","validate","job-3","interrupted").unwrap();let retry=recovery(&db,TaskArgs{workspace:"root".into(),project:"demo".into(),task_id:"task-1".into()}).unwrap();assert_eq!(retry["recommended_action"]["kind"],"inspect_and_retry");assert_eq!(retry["latest_job"]["job_id"],"job-3");}
    #[test]fn continue_without_task_id_selects_latest_project_task(){let d=tempfile::tempdir().unwrap();let db=Store::open(&d.path().join("db")).unwrap();record_job(&db,"root","demo","older","stage","job-old","succeeded").unwrap();record_job(&db,"root","demo","newer","stage","job-new","running").unwrap();let value=continue_recovery(&db,crate::tools::types::ContinueTaskArgs{workspace:"root".into(),project:"demo".into(),task_id:None}).unwrap();assert_eq!(value["resolved_task_id"],"newer");assert_eq!(value["selection"],"latest_explicit_task");assert_eq!(value["recommended_action"]["kind"],"poll_job");}
}
