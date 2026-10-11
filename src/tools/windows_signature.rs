// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: windows_signature.rs
// Description: Offline, embedded-only WinVerifyTrust diagnostics for the
// fixed Windows AppContainer worker. Catalog-signed Windows system images
// need a separate verification path; do not authorize arbitrary executables.

use anyhow::{bail,Context,Result};
use std::{
    ffi::c_void,
    fs::{File,OpenOptions},
    mem::size_of,
    os::windows::{ffi::OsStrExt,fs::{OpenOptionsExt,MetadataExt},io::AsRawHandle},
    path::Path,
};

const FILE_FLAG_OPEN_REPARSE_POINT:u32=0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT:u32=0x400;
const WTD_UI_NONE:u32=2;
const WTD_REVOKE_NONE:u32=0;
const WTD_CHOICE_FILE:u32=1;
const WTD_STATEACTION_VERIFY:u32=1;
const WTD_STATEACTION_CLOSE:u32=2;
const WTD_CACHE_ONLY_URL_RETRIEVAL:u32=0x1000;

#[repr(C)]
#[derive(Clone,Copy)]
struct Guid{data1:u32,data2:u16,data3:u16,data4:[u8;8]}
const GENERIC_VERIFY_V2:Guid=Guid{
    data1:0x00aac56b,data2:0xcd44,data3:0x11d0,
    data4:[0x8c,0xc2,0,0xc0,0x4f,0xc2,0x95,0xee],
};
#[repr(C)]
struct WintrustFileInfo{
    cb_struct:u32,
    file_path:*const u16,
    file:*mut c_void,
    known_subject:*const Guid,
}
#[repr(C)]
struct WintrustData{
    cb_struct:u32,
    policy_callback:*mut c_void,
    sip_callback:*mut c_void,
    ui_choice:u32,
    revocation_checks:u32,
    union_choice:u32,
    union_info:*mut c_void,
    state_action:u32,
    state:*mut c_void,
    url_reference:*mut u16,
    flags:u32,
    ui_context:u32,
    signature_settings:*mut c_void,
}
#[link(name="wintrust")]
unsafe extern "system"{
    fn WinVerifyTrust(window:*mut c_void,action:*const Guid,data:*mut WintrustData)->i32;
}

/// A verified embedded chain does not automatically pin a publisher identity.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(crate) enum EmbeddedTrust{
    TrustedEmbedded,
    NotTrustedOrCatalogOnly,
}
impl EmbeddedTrust{
    pub(crate) fn label(self)->&'static str{
        match self{
            Self::TrustedEmbedded=>"embedded_chain_trusted",
            Self::NotTrustedOrCatalogOnly=>"embedded_not_trusted_or_catalog_only",
        }
    }
}
fn open_without_reparse(path:&Path)->Result<File>{
    if !path.is_absolute(){bail!("Signature verification requires an absolute file path");}
    let mut options=OpenOptions::new();
    options.read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file=options.open(path).context("Open Windows image for offline signature verification")?;
    if !file.metadata()?.is_file()||
        file.metadata()?.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{
        bail!("Signature verification refuses symlinks and reparse-point files");
    }
    Ok(file)
}
fn verify_handle(file:&File,path:&Path)->Result<EmbeddedTrust>{
    let mut utf16=path.as_os_str().encode_wide().collect::<Vec<_>>();
    if utf16.is_empty()||utf16.contains(&0){bail!("Invalid Windows signature-verification path");}
    utf16.push(0);
    let mut file_info=WintrustFileInfo{
        cb_struct:size_of::<WintrustFileInfo>() as u32,
        file_path:utf16.as_ptr(),
        file:file.as_raw_handle() as _,
        known_subject:std::ptr::null(),
    };
    let mut trust=WintrustData{
        cb_struct:size_of::<WintrustData>() as u32,
        policy_callback:std::ptr::null_mut(),
        sip_callback:std::ptr::null_mut(),
        ui_choice:WTD_UI_NONE,
        revocation_checks:WTD_REVOKE_NONE,
        union_choice:WTD_CHOICE_FILE,
        union_info:&mut file_info as *mut _ as *mut c_void,
        state_action:WTD_STATEACTION_VERIFY,
        state:std::ptr::null_mut(),
        url_reference:std::ptr::null_mut(),
        flags:WTD_CACHE_ONLY_URL_RETRIEVAL,
        ui_context:0,
        signature_settings:std::ptr::null_mut(),
    };
    let status=unsafe{WinVerifyTrust(std::ptr::null_mut(),&GENERIC_VERIFY_V2,&mut trust)};
    // Providers can allocate state even when the signature fails verification.
    trust.state_action=WTD_STATEACTION_CLOSE;
    let _=unsafe{WinVerifyTrust(std::ptr::null_mut(),&GENERIC_VERIFY_V2,&mut trust)};
    Ok(if status==0{EmbeddedTrust::TrustedEmbedded}
        else{EmbeddedTrust::NotTrustedOrCatalogOnly})
}
pub(crate) fn verify_embedded(path:&Path)->Result<EmbeddedTrust>{
    let file=open_without_reparse(path)?;
    verify_handle(&file,path)
}

/// Diagnostics are limited to approved System32 MSVC-runtime basenames.
/// A DLL can be catalog-signed without an embedded signature. This function
/// cannot treat an unverified embedded result as malicious or trusted.
pub(crate) fn runtime_diagnostics(modules:&[&str])->Result<serde_json::Value>{
    let system=crate::tools::windows_trusted_worker::system32_directory()?;
    if modules.len()>4{bail!("Too many runtime signature probes");}
    let mut checked=Vec::with_capacity(modules.len());
    for name in modules{
        if !matches!(*name,
            "vcruntime140.dll"|"vcruntime140_1.dll"|"msvcp140.dll"|"concrt140.dll"){
            bail!("Only preapproved System32 runtime DLLs may be verified");
        }
        let path=system.join(name);
        let verdict=verify_embedded(&path)
            .with_context(||format!("Verify offline embedded trust for runtime {name}"))?;
        checked.push(serde_json::json!({
            "module":name,
            "embedded_verdict":verdict.label(),
            "requires_catalog_validation":verdict!=EmbeddedTrust::TrustedEmbedded,
        }));
    }
    Ok(serde_json::json!({
        "checks":checked,
        "offline":true,
        "winverifytrust_action":"WINTRUST_ACTION_GENERIC_VERIFY_V2",
        "revocation_network_access":false,
        "catalog_signatures_checked":false,
        "trusted_publisher_pinned":false,
        "execution_gate":false,
    }))
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn wintrust_layout_and_offline_flags_are_correct(){
        assert_eq!(size_of::<Guid>(),16);
        assert_eq!(size_of::<WintrustFileInfo>(),32);
        assert_eq!(size_of::<WintrustData>(),88);
        assert_eq!(WTD_CACHE_ONLY_URL_RETRIEVAL,0x1000);
        assert!(matches!(EmbeddedTrust::TrustedEmbedded.label(),
            "embedded_chain_trusted"));
    }
    #[test]
    fn unsigned_file_cannot_be_trusted_and_nonfiles_fail_closed(){
        let root=tempfile::tempdir().unwrap();
        let file=root.path().join("unsigned.exe");
        std::fs::write(&file,b"not an executable or signature").unwrap();
        assert_eq!(verify_embedded(&file).unwrap(),EmbeddedTrust::NotTrustedOrCatalogOnly);
        assert!(verify_embedded(&root.path().join("missing.dll")).is_err());
        assert!(verify_embedded(Path::new("relative.dll")).is_err());
    }
    #[test]
    fn approved_system32_runtime_has_a_non_authorizing_signature_diagnostic(){
        let result=runtime_diagnostics(&["vcruntime140.dll"]).unwrap();
        assert_eq!(result["offline"],true);
        assert_eq!(result["catalog_signatures_checked"],false);
        assert_eq!(result["execution_gate"],false);
        assert_eq!(result["trusted_publisher_pinned"],false);
        let list=result["checks"].as_array().unwrap();
        assert_eq!(list.len(),1);
        assert_eq!(list[0]["module"],"vcruntime140.dll");
        assert!(matches!(list[0]["embedded_verdict"].as_str().unwrap(),
            "embedded_chain_trusted"|"embedded_not_trusted_or_catalog_only"));
        assert!(runtime_diagnostics(&["fake.dll"]).is_err());
        assert!(runtime_diagnostics(&["../vcruntime140.dll"]).is_err());
    }
}
