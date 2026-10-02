use serde_json::{json,Value};
use std::{error::Error as StdError,fmt};

#[derive(Debug)]
pub struct CodedError{
    code:&'static str,
    retryable:bool,
    message:String,
    details:Value,
}
impl CodedError{
    pub fn new(code:&'static str,retryable:bool,message:impl Into<String>,details:Value)->Self{Self{code,retryable,message:message.into(),details}}
    pub fn code(&self)->&'static str{self.code}
    pub fn retryable(&self)->bool{self.retryable}
    pub fn details(&self)->&Value{&self.details}
}
impl fmt::Display for CodedError{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str(&self.message)}}
impl StdError for CodedError{}

pub fn coded(code:&'static str,retryable:bool,message:impl Into<String>)->anyhow::Error{CodedError::new(code,retryable,message,json!({})).into()}
pub fn coded_details(code:&'static str,retryable:bool,message:impl Into<String>,details:Value)->anyhow::Error{CodedError::new(code,retryable,message,details).into()}
pub fn payload(error:&anyhow::Error)->Value{
    if let Some(coded)=error.downcast_ref::<CodedError>(){
        return json!({"code":coded.code(),"message":format!("{error:#}"),"retryable":coded.retryable(),"details":coded.details()});
    }
    json!({"code":"OPERATION_FAILED","message":format!("{error:#}"),"retryable":false,"details":{}})
}
pub fn code(error:&anyhow::Error)->&'static str{error.downcast_ref::<CodedError>().map(CodedError::code).unwrap_or("OPERATION_FAILED")}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn coded_error_keeps_code_separate_from_message(){let error=coded_details("FILE_CONFLICT",true,"content changed",json!({"path":"README.md"}));let value=payload(&error);assert_eq!(value["code"],"FILE_CONFLICT");assert_eq!(value["message"],"content changed");assert_eq!(value["retryable"],true);assert_eq!(value["details"]["path"],"README.md");}
    #[test]fn unknown_error_has_stable_fallback(){let error=anyhow::anyhow!("plain failure");let value=payload(&error);assert_eq!(value["code"],"OPERATION_FAILED");assert_eq!(value["retryable"],false);}
}
