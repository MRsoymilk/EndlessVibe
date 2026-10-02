use anyhow::{bail,Result};
use serde::{Deserialize,Serialize};
use std::fmt;

#[derive(Clone,Copy,Debug,Eq,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum TaskState{
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskState{
    pub fn can_transition(self,next:Self)->bool{
        matches!((self,next),
            (Self::Pending,Self::Running)|
            (Self::Pending,Self::Cancelled)|
            (Self::Running,Self::Succeeded)|
            (Self::Running,Self::Failed)|
            (Self::Running,Self::Cancelled)|
            (Self::Running,Self::Interrupted)|
            (Self::Succeeded,Self::Succeeded)|
            (Self::Failed,Self::Failed)|
            (Self::Cancelled,Self::Cancelled)|
            (Self::Interrupted,Self::Interrupted))
    }
    pub fn from_job(status:&str)->Result<Self>{match status{
        "queued"=>Ok(Self::Pending),"running"=>Ok(Self::Running),"succeeded"=>Ok(Self::Succeeded),"failed"=>Ok(Self::Failed),"cancelled"=>Ok(Self::Cancelled),"interrupted"=>Ok(Self::Interrupted),_=>bail!("unknown job state: {status}")}}
}
impl fmt::Display for TaskState{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{write!(f,"{}",serde_json::to_value(self).unwrap().as_str().unwrap())}}

#[cfg(test)]
mod tests{
 use super::*;
 #[test]fn valid_flow(){assert!(TaskState::Pending.can_transition(TaskState::Running));assert!(TaskState::Running.can_transition(TaskState::Succeeded));assert!(!TaskState::Succeeded.can_transition(TaskState::Running));}
 #[test]fn job_mapping(){assert_eq!(TaskState::from_job("queued").unwrap(),TaskState::Pending);assert!(TaskState::from_job("bad").is_err());}
}
