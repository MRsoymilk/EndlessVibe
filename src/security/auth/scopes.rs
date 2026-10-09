use super::{AResult,AuthError};

pub const SCOPES:&[&str]=&["projects:read","files:read","files:write","commands:execute","git:write","docker:read","docker:write"];

pub fn required_scopes(tool:&str)->&'static [&'static str]{
    match tool{
        "list_workspaces"|"list_projects"|"inspect_project"|"get_task_checkpoint"|"continue_task"|"list_task_checkpoints"=>&["projects:read"],
        "list_directory"|"read_file"|"search_code"|"git_status"|"git_diff"|"git_log"=>&["files:read"],
        "write_file"|"apply_patch"|"create_directory"=>&["files:write"],
        "run_command"|"run_shell"=>&["commands:execute","files:write"],
        "start_task"=>&["commands:execute"],
        "get_sandbox_diagnostics"|"get_job"|"get_job_output"|"cancel_job"|"list_jobs"=>&["commands:execute"],
        "git_commit"=>&["git:write","files:write"],
        "git_push"=>&["git:write"],
        "docker_list"|"docker_inspect"|"docker_logs"|"docker_stats"|"docker_compose"=>&["docker:read"],
        "docker_start"|"docker_stop"|"docker_restart"=>&["docker:write"],
        "hello"|"get_service_status"=>&[],
        _=>SCOPES,
    }
}

pub(super) fn parse(value:Option<&str>)->AResult<Vec<String>>{
    let values:std::collections::BTreeSet<String>=match value{
        Some(s)=>s.split_ascii_whitespace().map(str::to_owned).collect(),
        None=>SCOPES.iter().map(|s|(*s).into()).collect(),
    };
    if values.is_empty()||values.iter().any(|v|!SCOPES.contains(&v.as_str())){
        return Err(AuthError::bad("invalid_scope","Unknown or empty scope"));
    }
    Ok(values.into_iter().collect())
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn scope_escalation_is_not_possible_in_parser(){assert!(parse(Some("files:read root")).is_err());assert_eq!(parse(Some("files:read files:read")).unwrap(),vec!["files:read"]);}
    #[test]fn command_scopes_include_writes(){assert!(required_scopes("run_command").contains(&"files:write"));assert_eq!(required_scopes("get_sandbox_diagnostics"),&["commands:execute"]);assert_eq!(required_scopes("continue_task"),&["projects:read"]);assert_eq!(required_scopes("start_task"),&["commands:execute"]);assert!(required_scopes("git_commit").contains(&"git:write"));assert_eq!(required_scopes("git_push"),&["git:write"]);assert_eq!(required_scopes("docker_list"),&["docker:read"]);assert_eq!(required_scopes("docker_restart"),&["docker:write"]);}
}
