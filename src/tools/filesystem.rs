use crate::{runtime::Runtime, tools::types::*, util, workspace::Project};
use anyhow::{bail, Context, Result};
use regex::RegexBuilder;
use serde_json::{json, Value};
use std::collections::VecDeque;

pub fn list(w: &Project, a: DirectoryArgs) -> Result<Value> {
    if a.limit == 0 || a.limit > 1000 { bail!("limit must be 1..1000"); }
    let entries=w.root.entries(&a.path)?; let total=entries.len();
    let selected=entries.into_iter().skip(a.offset).take(a.limit).map(|e|json!({"name":e.name,"type":e.kind,"bytes":e.bytes})).collect::<Vec<_>>();
    Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"path":a.path,"entries":selected,"total":total,"next_offset":if a.offset.saturating_add(a.limit)<total {Some(a.offset+a.limit)} else {None}}))
}
pub fn read(rt: &Runtime, w: &Project, a: ReadArgs) -> Result<Value> {
    if a.start_line == 0 || a.max_lines == 0 || a.max_lines>5000 { bail!("start_line >= 1 and max_lines 1..5000 are required"); }
    let bytes=w.root.read(&a.path,rt.config.limits.max_file_bytes)?;
    let content=std::str::from_utf8(&bytes).context("File is not UTF-8 text")?; if content.contains('\0') { bail!("Binary files are not returned as text"); }
    let lines=content.split_inclusive('\n').collect::<Vec<_>>(); let mut selected=String::new(); let mut count=0;
    for line in lines.iter().skip(a.start_line-1).take(a.max_lines) { if selected.len()+line.len()>rt.config.limits.max_read_bytes { if count==0 { bail!("A single line exceeds max_read_bytes; increase the configured limit locally"); } break; } selected.push_str(line); count+=1; }
    let end=(a.start_line-1).saturating_add(count);
    Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"path":a.path,"sha256":util::digest(&bytes),"size_bytes":bytes.len(),"total_lines":lines.len(),"start_line":a.start_line,"lines_returned":count,"content":selected,"next_line":if end<lines.len(){Some(end+1)}else{None}}))
}
fn check_expected(existing: Option<&[u8]>, expected: &str) -> Result<()> {
    match existing { None if expected=="MISSING"=>Ok(()),Some(b) if expected==util::digest(b)=>Ok(()),_=>bail!("FILE_CONFLICT: expected_sha256 does not match current content; read again, do not overwrite") }
}
pub fn write(rt: &Runtime, w: &Project, a: WriteArgs) -> Result<Value> {
    w.write_allowed()?;
    if a.content.len()>rt.config.limits.max_file_bytes || a.content.contains('\0') { bail!("Content must be bounded UTF-8 text without NUL"); }
    let existing=w.root.read_optional(&a.path,rt.config.limits.max_file_bytes)?; check_expected(existing.as_deref(),&a.expected_sha256)?;
    let new_hash=util::digest(a.content.as_bytes());
    if existing.as_deref()==Some(a.content.as_bytes()) { return Ok(json!({"path":a.path,"sha256":new_hash,"changed":false})); }
    let backup=if let Some(old)=&existing { let id=util::random_secret()?; util::private_create(&rt.config.security.data_dir.join("backups").join(&id),old)?; Some(id) } else {None};
    // Compare again immediately before rename. External editors do not share our lock;
    // this is optimistic conflict detection, not a filesystem compare-and-swap primitive.
    check_expected(w.root.read_optional(&a.path,rt.config.limits.max_file_bytes)?.as_deref(),&a.expected_sha256)?;
    w.root.replace(&a.path,a.content.as_bytes(),existing.is_none(),a.create_parents)?;
    Ok(json!({"workspace":w.workspace_id,"project":w.config.id,"path":a.path,"changed":true,"sha256":new_hash,"previous_sha256":existing.as_ref().map(util::digest),"backup_id":backup,"bytes":a.content.len()}))
}
pub fn patch(rt: &Runtime, w: &Project, a: PatchArgs) -> Result<Value> {
    w.write_allowed()?; if a.edits.is_empty() || a.edits.len()>128 { bail!("edits must contain 1..128 exact replacements"); }
    let bytes=w.root.read(&a.path,rt.config.limits.max_file_bytes)?; check_expected(Some(&bytes),&a.expected_sha256)?;
    let mut text=String::from_utf8(bytes).context("Not UTF-8")?;
    for edit in a.edits { if edit.old_text.is_empty() || edit.expected_occurrences==0 || edit.expected_occurrences>1000 { bail!("Each edit needs nonempty old_text and expected_occurrences 1..1000"); } let found=text.matches(edit.old_text.as_str()).count(); if found!=edit.expected_occurrences { bail!("PATCH_CONFLICT: expected {} occurrences, found {found}",edit.expected_occurrences); } let removed=found.checked_mul(edit.old_text.len()).context("Patch size overflow")?;let added=found.checked_mul(edit.new_text.len()).context("Patch size overflow")?;let length=text.len().checked_sub(removed).and_then(|v|v.checked_add(added)).context("Patch size overflow")?;if length>rt.config.limits.max_file_bytes{bail!("Patch result is too large");}text=text.replace(edit.old_text.as_str(),edit.new_text.as_str()); }
    write(rt,w,WriteArgs{workspace:a.workspace,project:a.project,path:a.path,content:text,expected_sha256:a.expected_sha256,create_parents:false})
}
pub fn mkdir(w:&Project,a:MakeDirectoryArgs)->Result<Value>{w.write_allowed()?;w.root.create_directory(&a.path)?;Ok(json!({"path":a.path,"created_or_exists":true}))}
pub fn search(rt:&Runtime,w:&Project,a:SearchArgs)->Result<Value>{
    if a.query.is_empty() || a.query.len()>4096 || a.max_results==0 || a.max_results>1000 {bail!("query must be 1..4096 bytes and max_results 1..1000");}
    let pattern=if a.regex{a.query.clone()}else{regex::escape(&a.query)};
    let expression=RegexBuilder::new(&pattern).case_insensitive(!a.case_sensitive).size_limit(2*1024*1024).build().context("Invalid or oversized regex")?;
    let mut queue=VecDeque::from([(a.path,0usize)]);let mut results=Vec::new();let mut visited=0;let mut bytes:usize=0;let mut skipped=0;let mut truncated=false;let mut result_bytes:usize=0;
    'outer:while let Some((directory,depth))=queue.pop_front(){
        if depth>32{truncated=true;continue;}
        for entry in w.root.entries(&directory)?{
            if ["target","build","node_modules",".venv","venv",".cache"].contains(&entry.name.as_str()){continue;}
            let path=if directory=="."{entry.name}else{format!("{directory}/{}",entry.name)};
            if entry.kind=="directory"{if queue.len()<rt.config.limits.search_max_files{queue.push_back((path,depth+1));}else{truncated=true;}continue;}
            if entry.kind!="file"{skipped+=1;continue;}
            visited+=1;if visited>rt.config.limits.search_max_files||bytes>=rt.config.limits.search_max_bytes{truncated=true;break 'outer;}
            let data=match w.root.read(&path,rt.config.limits.max_file_bytes){Ok(v)=>v,Err(_)=>{skipped+=1;continue;}};if bytes.saturating_add(data.len())>rt.config.limits.search_max_bytes{truncated=true;break 'outer;}bytes+=data.len();
            let text=match std::str::from_utf8(&data){Ok(s) if !s.contains('\0')=>s,_=>{skipped+=1;continue;}};
            for (line,content) in text.lines().enumerate(){if expression.is_match(content){let hit=json!({"path":path,"line":line+1,"text":util::bounded_text(content,2048)});let hit_bytes=hit.to_string().len();if result_bytes+hit_bytes>rt.config.limits.max_output_bytes{truncated=true;break 'outer;}result_bytes+=hit_bytes;results.push(hit);if results.len()>=a.max_results{truncated=true;break 'outer;}}}
        }
    }
    Ok(json!({"matches":results,"files_examined":visited,"bytes_examined":bytes,"skipped":skipped,"truncated":truncated}))
}
#[cfg(test)] mod tests { use super::*; #[test] fn hashes_are_required_for_overwrites(){assert!(check_expected(None,"MISSING").is_ok());assert!(check_expected(Some(b"old"),"MISSING").is_err());assert!(check_expected(Some(b"old"),&util::digest("old")).is_ok());assert!(check_expected(Some(b"changed"),&util::digest("old")).is_err());} }
