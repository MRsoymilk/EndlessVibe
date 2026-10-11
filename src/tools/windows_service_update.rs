// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: windows_service_update.rs
// Description: Exact-path Windows self-update preflight. This module must not
// turn an MCP request into a general host program runner.

use anyhow::{bail,Context,Result};
use serde::{Deserialize,Serialize};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::{
    fs::{self,File,OpenOptions},
    io::Read,
    os::windows::fs::{MetadataExt,OpenOptionsExt},
    path::{Path,PathBuf},
};

pub(crate) const MAX_EXE_BYTES:u64=128*1024*1024;
const OPEN_REPARSE_POINT:u32=0x0020_0000;
const REPARSE_POINT:u32=0x400;
const STAGED_EXE:&str="target/windows-stage/debug/endlessvibe.exe";

#[derive(Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstalledManifest{
    pub(crate) sha256:String,
    #[serde(default)]
    pub(crate) config_path:Option<String>,
}
#[derive(Debug)]
pub(crate) struct Installation{
    pub(crate) root:PathBuf,
    pub(crate) manifest:PathBuf,
    pub(crate) running:PathBuf,
    pub(crate) sha256:String,
    pub(crate) config_path:Option<String>,
}
pub(crate) fn digest_ok(value:&str)->bool{
    value.len()==64 && value.bytes().all(|v|v.is_ascii_hexdigit())
}
pub(crate) fn regular_file(path:&Path)->Result<File>{
    let metadata=fs::symlink_metadata(path)
        .with_context(||format!("Inspect Windows executable {}",path.display()))?;
    if !metadata.is_file()||metadata.file_attributes()&REPARSE_POINT!=0{
        bail!("Windows update executable must be a non-reparse regular file");
    }
    let mut options=OpenOptions::new();
    options.read(true).custom_flags(OPEN_REPARSE_POINT);
    let file=options.open(path)?;
    if !file.metadata()?.is_file()||file.metadata()?.file_attributes()&REPARSE_POINT!=0{
        bail!("Windows update executable changed while opening");
    }
    Ok(file)
}
pub(crate) fn fingerprint(path:&Path)->Result<(String,Vec<u8>)>{
    let file=regular_file(path)?;
    let length=file.metadata()?.len();
    if length==0||length>MAX_EXE_BYTES{bail!("Windows update EXE must be 1..128 MiB");}
    let mut data=Vec::with_capacity(length as usize);
    file.take(MAX_EXE_BYTES+1).read_to_end(&mut data)?;
    if data.len() as u64!=length{bail!("Windows update EXE changed during hashing");}
    Ok((format!("{:x}",Sha256::digest(&data)),data))
}
pub(crate) fn real_directory(path:&Path)->Result<()>{
    let m=fs::symlink_metadata(path)?;
    if !m.is_dir()||m.file_attributes()&REPARSE_POINT!=0{
        bail!("Windows update directory must not be a junction or symlink");
    }
    Ok(())
}
pub(crate) fn allowed_candidate(project:&Path)->Result<PathBuf>{
    let mut current=project.to_owned();
    real_directory(&current)?;
    for name in ["target","windows-stage","debug"]{
        current.push(name);
        real_directory(&current)
            .with_context(||format!("Windows Cargo staging directory {name} is not trusted"))?;
    }
    let source=current.join("endlessvibe.exe");
    let canonical=fs::canonicalize(&source)?;
    if !canonical.starts_with(fs::canonicalize(project)?){
        bail!("Windows staged update source escapes the granted Project");
    }
    regular_file(&source)?;
    Ok(source)
}
pub(crate) fn installation_from_executable(running:&Path)->Result<Installation>{
    let running=fs::canonicalize(running)?;
    if running.file_name().and_then(|n|n.to_str())!=Some("endlessvibe.exe"){
        bail!("Windows updater requires the installed EndlessVibe binary");
    }
    let version=running.parent().context("Missing EndlessVibe release version")?;
    let sha=version.file_name().and_then(|n|n.to_str())
        .context("Windows release version is not an ASCII digest")?;
    if !digest_ok(sha){bail!("Windows installed release directory is not SHA-256 named");}
    let releases=version.parent().context("Missing EndlessVibe releases directory")?;
    if releases.file_name().and_then(|n|n.to_str())!=Some("releases"){
        bail!("Windows updater requires a versioned releases directory");
    }
    let root=releases.parent().context("Missing EndlessVibe install root")?;
    if root.file_name().and_then(|n|n.to_str())!=Some("EndlessVibe")
       ||root.parent().and_then(|n|n.file_name()).and_then(|n|n.to_str())!=Some("Programs"){
        bail!("Windows updater refuses an installation outside Programs/EndlessVibe");
    }
    for directory in [root,releases,version]{real_directory(directory)?;}
    let manifest_path=root.join("current.json");
    let manifest_bytes=crate::util::private_read(&manifest_path,4096)?;
    let manifest:InstalledManifest=serde_json::from_slice(&manifest_bytes)
        .context("Read verified Windows installation manifest")?;
    if !digest_ok(&manifest.sha256)||!sha.eq_ignore_ascii_case(&manifest.sha256){
        bail!("Windows installation manifest and running EXE release directory disagree");
    }
    let (running_digest,_)=fingerprint(&running)?;
    if running_digest!=manifest.sha256.to_ascii_lowercase(){
        bail!("Windows installation manifest and running EXE bytes disagree");
    }
    Ok(Installation{root:root.to_owned(),manifest:manifest_path,
        running,sha256:running_digest,config_path:manifest.config_path})
}
pub fn plan_for_project(
    rt:&crate::runtime::Runtime,
    project:&crate::workspace::Project,
)->Result<Value>{
    project.exec_allowed()?;
    if !rt.config.security.allow_windows_service_update{
        return Ok(json!({
            "enabled":false,
            "reason":"security.allow_windows_service_update is false",
            "execution_started":false,
        }));
    }
    let allowed=rt.config.security.windows_update_project.as_ref()
        .context("Windows update Project is not configured")?;
    if fs::canonicalize(allowed)?!=fs::canonicalize(&project.root.path)?{
        bail!("Windows update source is not the explicitly configured Project");
    }
    project.root.unchanged_root()?;
    let install=installation_from_executable(&std::env::current_exe()?)?;
    let candidate=allowed_candidate(&project.root.path)?;
    let (digest,bytes)=fingerprint(&candidate)?;
    let inspected=crate::tools::windows_pe::inspect_pe_imports(&bytes)
        .context("Refuse Windows update whose PE import policy was not verified")?;
    if inspected.machine!=0x8664||inspected.has_delay_load{
        bail!("Only audited AMD64 PE32+ executables may be deployed");
    }
    Ok(json!({
        "enabled":true,
        "source":STAGED_EXE,
        "current_sha256":install.sha256,
        "candidate_sha256":digest,
        "candidate_bytes":bytes.len(),
        "same_binary":digest==install.sha256,
        "pe_machine":"amd64",
        "import_count":inspected.modules.len(),
        "source_project":project.config.id,
        "execution_started":false,
        "shell_access_granted":false,
        "explicit_hash_and_confirmation_required":true,
    }))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn default_deny_and_invalid_installations(){
        assert!(digest_ok(&"a".repeat(64)));
        for digest in ["", "g".repeat(64).as_str(), "f".repeat(63).as_str()]{
            assert!(!digest_ok(digest));
        }
        let root=tempfile::tempdir().unwrap();
        assert!(installation_from_executable(&root.path().join("anything.exe")).is_err());
        assert!(allowed_candidate(root.path()).is_err());
        assert!(fingerprint(&root.path().join("missing.exe")).is_err());
        let empty=root.path().join("empty.exe");
        fs::write(&empty,b"").unwrap();
        assert!(fingerprint(&empty).is_err());
    }
    #[test]
    fn fixed_stage_source_and_reparse_dirs_are_enforced(){
        let root=tempfile::tempdir().unwrap();
        let stage=root.path().join("target/windows-stage/debug");
        fs::create_dir_all(&stage).unwrap();
        let exe=stage.join("endlessvibe.exe");
        fs::write(&exe,b"mock executable").unwrap();
        assert_eq!(allowed_candidate(root.path()).unwrap(),exe);
        assert!(allowed_candidate(&stage).is_err());
        let outside=root.path().join("other.exe");
        fs::write(&outside,b"not in stage").unwrap();
        assert!(fingerprint(&outside).is_ok(),
            "Only allowed_candidate should authorize which artifact is usable");
    }
    #[test]
    fn exact_install_path_layout_is_required(){
        let dir=tempfile::tempdir().unwrap();
        let fake=dir.path().join("Programs/EndlessVibe/releases/garbage/endlessvibe.exe");
        fs::create_dir_all(fake.parent().unwrap()).unwrap();
        fs::write(&fake,b"fake").unwrap();
        assert!(installation_from_executable(&fake).is_err());
    }
}
