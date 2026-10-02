use schemars::JsonSchema;
use serde::{Deserialize,Serialize};
use std::collections::BTreeMap;

#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct WorkspaceArgs{#[serde(default)] pub workspace: String}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct ProjectArgs{pub workspace:String,#[serde(default)] pub project: String}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct DirectoryArgs{pub workspace:String,#[serde(default)] pub project: String,#[serde(default="root_path")]pub path:String,#[serde(default)]pub offset:usize,#[serde(default="page_size")]pub limit:usize}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct ReadArgs{pub workspace:String,#[serde(default)] pub project: String,pub path:String,#[serde(default="first_line")]pub start_line:usize,#[serde(default="page_size")]pub max_lines:usize}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct WriteArgs{pub workspace:String,#[serde(default)] pub project: String,pub path:String,pub content:String,/// SHA-256 from read_file; use the literal MISSING only when creating a new file.
    pub expected_sha256:String,#[serde(default)]pub create_parents:bool}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct Edit{pub old_text:String,pub new_text:String,#[serde(default="one")]pub expected_occurrences:usize}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct PatchArgs{pub workspace:String,#[serde(default)] pub project: String,pub path:String,pub expected_sha256:String,pub edits:Vec<Edit>}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct MakeDirectoryArgs{pub workspace:String,#[serde(default)] pub project: String,pub path:String}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct SearchArgs{pub workspace:String,#[serde(default)] pub project: String,pub query:String,#[serde(default="root_path")]pub path:String,#[serde(default)]pub regex:bool,#[serde(default="yes")]pub case_sensitive:bool,#[serde(default="page_size")]pub max_results:usize}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct CommandArgs{pub workspace:String,#[serde(default)] pub project: String,pub program:String,#[serde(default)]pub args:Vec<String>,#[serde(default="root_path")]pub cwd:String,pub request_id:String,pub timeout_seconds:Option<u64>,#[serde(default)]pub preflight_programs:Vec<String>,#[serde(default)]pub environment:BTreeMap<String,String>,#[serde(default)]pub network:bool,#[serde(default)]pub task_id:Option<String>,#[serde(default)]pub stage:Option<String>}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct ShellArgs{pub workspace:String,#[serde(default)] pub project: String,pub script:String,#[serde(default="root_path")]pub cwd:String,pub request_id:String,pub timeout_seconds:Option<u64>,#[serde(default)]pub preflight_programs:Vec<String>,#[serde(default)]pub environment:BTreeMap<String,String>,#[serde(default)]pub network:bool,#[serde(default)]pub task_id:Option<String>,#[serde(default)]pub stage:Option<String>}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct JobArgs{pub job_id:String}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct OutputArgs{pub job_id:String,#[serde(default)]pub offset:u64,#[serde(default="output_limit")]pub limit:usize}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct ListJobsArgs{pub workspace:Option<String>,pub project:Option<String>,#[serde(default)]pub task_id:Option<String>,#[serde(default="twenty")]pub limit:usize}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct DiffArgs{pub workspace:String,#[serde(default)] pub project: String,#[serde(default)]pub paths:Vec<String>}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct LogArgs{pub workspace:String,#[serde(default)] pub project: String,#[serde(default="twenty")]pub limit:usize}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct CommitArgs{pub workspace:String,#[serde(default)] pub project: String,pub paths:Vec<String>,pub message:String,/// Exact head returned by git_diff (UNBORN for first commit).
    pub expected_head:String,/// Exact SHA-256 review token returned by git_diff for these paths.
    pub expected_diff_sha256:String,#[serde(default)]pub task_id:Option<String>,#[serde(default)]pub stage:Option<String>}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct PushArgs{pub workspace:String,#[serde(default)]pub project:String,pub remote:String,pub branch:String,/// Exact local branch commit expected by the caller; push is refused if it changed.
    pub expected_head:String}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct TaskArgs{pub workspace:String,#[serde(default)]pub project:String,pub task_id:String}
#[derive(Clone,Debug,Deserialize,Serialize,JsonSchema)]#[serde(deny_unknown_fields)]
pub struct ListTaskCheckpointsArgs{pub workspace:Option<String>,pub project:Option<String>,#[serde(default)]pub task_id:Option<String>,#[serde(default="twenty")]pub limit:usize}
fn root_path()->String{".".into()}fn page_size()->usize{200}fn first_line()->usize{1}fn one()->usize{1}fn yes()->bool{true}fn twenty()->usize{20}fn output_limit()->usize{65536}
