import {$,node} from "./common.js";

const toolGroups=[
  ["连接",["hello","get_service_status"]],
  ["Workspace / Project",["list_workspaces","list_projects","inspect_project"]],
  ["文件",["list_directory","read_file","write_file","apply_patch","create_directory","search_code"]],
  ["命令",["run_command","run_shell"]],
  ["任务",["get_job","get_job_output","cancel_job","list_jobs","start_task","get_task_checkpoint","list_task_checkpoints"]],
  ["Git",["git_status","git_diff","git_log","git_commit","git_push"]],
  ["Docker",["docker_list","docker_inspect","docker_logs","docker_stats","docker_compose","docker_start","docker_stop","docker_restart"]]
];

export function renderToolTree(id,tools){
  const host=$(id);if(!host)return;
  host.replaceChildren();
  const remaining=new Set(tools||[]);
  if(!remaining.size){host.className="tool-tree empty-state";host.textContent="暂无工具";return;}
  host.className="tool-tree";
  for(const [label,names] of toolGroups){
    const present=names.filter(name=>remaining.delete(name));
    if(!present.length)continue;
    host.appendChild(group(label,present));
  }
  if(remaining.size)host.appendChild(group("其他",[...remaining].sort()));
}

function group(label,names){
  const details=node("details","tool-tree-group");details.open=true;
  const summary=node("summary","tool-tree-summary"),title=node("span","tool-tree-title",label),count=node("span","tool-tree-count",`${names.length}`);
  summary.append(title,count);
  const list=node("div","tool-tree-children");
  names.forEach((name,index)=>{
    const item=node("div","tool-tree-item"),branch=node("span","tool-tree-branch",index===names.length-1?"└─":"├─"),code=node("code","",name);
    item.append(branch,code);list.appendChild(item);
  });
  details.append(summary,list);
  return details;
}
