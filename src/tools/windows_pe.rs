// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: windows_pe.rs
// Description: Bounded, fail-closed audit of 64-bit Windows PE imports.
// Only the trusted self-worker uses this: it does NOT authorize arbitrary EXEs.

use anyhow::{bail,Context,Result};
use std::collections::BTreeSet;

const MAX_PE_BYTES:usize=128*1024*1024;
const MAX_SECTIONS:usize=96;
const MAX_DESCRIPTORS:usize=256;
const MAX_IMPORT_NAME:usize=128;
const MAX_IMPORT_DIRECTORY:usize=16*1024;

#[derive(Debug,Clone,PartialEq,Eq)]
pub(crate) struct PeImports {
    pub(crate) machine:u16,
    pub(crate) modules:Vec<String>,
    pub(crate) msvc_runtime_modules:Vec<String>,
    pub(crate) has_delay_load:bool,
}
#[derive(Clone,Copy)]
struct Section{rva:usize,virtual_size:usize,raw_offset:usize,raw_size:usize}
fn span(data:&[u8],offset:usize,len:usize)->Result<&[u8]>{
    let end=offset.checked_add(len).context("PE offset integer overflow")?;
    data.get(offset..end).context("PE structure exceeds file length")
}
fn u16le(data:&[u8],offset:usize)->Result<usize>{
    let bytes=span(data,offset,2)?;
    Ok(u16::from_le_bytes([bytes[0],bytes[1]]) as usize)
}
fn u32le(data:&[u8],offset:usize)->Result<usize>{
    let bytes=span(data,offset,4)?;
    Ok(u32::from_le_bytes([bytes[0],bytes[1],bytes[2],bytes[3]]) as usize)
}
fn rva_offset(data:&[u8],sections:&[Section],headers:usize,rva:usize,len:usize)->Result<usize>{
    let end=rva.checked_add(len).context("PE RVA overflow")?;
    if end<=headers{
        span(data,rva,len)?;
        return Ok(rva);
    }
    for section in sections{
        let reach=section.virtual_size.max(section.raw_size);
        if rva>=section.rva && rva<section.rva.saturating_add(reach){
            let inner=rva-section.rva;
            if inner.checked_add(len).is_some_and(|n|n<=section.raw_size){
                let offset=section.raw_offset.checked_add(inner)
                    .context("PE section offset overflow")?;
                span(data,offset,len)?;
                return Ok(offset);
            }
        }
    }
    bail!("PE import RVA does not resolve into initialized image bytes")
}
fn imported_name(data:&[u8],sections:&[Section],headers:usize,rva:usize)->Result<String>{
    // Check the RVA mapping for *every byte* of the DLL name. Checking
    // only its first byte could incorrectly accept names crossing section
    // boundaries into unmapped or unrelated bytes in the file.
    let mut bytes=Vec::with_capacity(MAX_IMPORT_NAME);
    let mut ended=false;
    for index in 0..MAX_IMPORT_NAME{
        let at=rva.checked_add(index).context("PE import-name RVA overflow")?;
        let pos=rva_offset(data,sections,headers,at,1)?;
        let byte=data[pos];
        if byte==0{ended=true;break;}
        bytes.push(byte);
    }
    if !ended{bail!("PE import name is not terminated in mapped image bytes");}
    if bytes.is_empty(){bail!("Empty PE module import");}
    let value=std::str::from_utf8(&bytes)
        .context("PE import name is not valid ASCII/UTF-8")?;
    if !value.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-'|b'.')){
        bail!("PE import contains path characters or invalid DLL name");
    }
    if !value.to_ascii_lowercase().ends_with(".dll"){
        bail!("PE import is not a DLL filename");
    }
    Ok(value.to_ascii_lowercase())
}
/// This allowlist is a *name preflight*, not proof of Windows loader identity.
/// Each module is an expected Windows OS DLL or a Windows API-set contract.
fn msvc_runtime_module(name:&str)->bool{
    matches!(name,"vcruntime140.dll"|"vcruntime140_1.dll"|"msvcp140.dll"|"concrt140.dll")
}
fn system_module(name:&str)->bool{
    let contract=(name.starts_with("api-ms-win-")||name.starts_with("ext-ms-win-"))
        &&name.ends_with(".dll");
    contract||matches!(name,
        "advapi32.dll"|"bcrypt.dll"|"bcryptprimitives.dll"|"cfgmgr32.dll"|
        "combase.dll"|"comctl32.dll"|"comdlg32.dll"|"crypt32.dll"|
        "cryptbase.dll"|"cryptsp.dll"|"dbghelp.dll"|"dnsapi.dll"|
        "gdi32.dll"|"gdi32full.dll"|"imm32.dll"|"iphlpapi.dll"|
        "kernel32.dll"|"kernelbase.dll"|"mpr.dll"|"msvcrt.dll"|
        "ncrypt.dll"|"netapi32.dll"|"normaliz.dll"|"ntdll.dll"|
        "ole32.dll"|"oleaut32.dll"|"powrprof.dll"|"profapi.dll"|
        "psapi.dll"|"rpcrt4.dll"|"secur32.dll"|"sechost.dll"|
        "setupapi.dll"|"shell32.dll"|"shlwapi.dll"|"sspicli.dll"|
        "ucrtbase.dll"|"user32.dll"|"userenv.dll"|"version.dll"|
        "winhttp.dll"|"wininet.dll"|"winmm.dll"|"winspool.drv"|
        "ws2_32.dll"|"wtsapi32.dll"|"windows.storage.dll"|"wintrust.dll")
}
pub(crate) fn inspect_pe_imports(data:&[u8])->Result<PeImports>{
    if data.len()<512||data.len()>MAX_PE_BYTES{
        bail!("Trusted worker PE size is invalid");
    }
    if span(data,0,2)?!=b"MZ"{bail!("Worker is not a Windows MZ executable");}
    let pe=u32le(data,0x3c)?;
    if span(data,pe,4)?!=b"PE\0\0"{bail!("Worker has invalid PE signature");}
    let coff=pe.checked_add(4).context("PE header offset overflow")?;
    let machine=u16le(data,coff)? as u16;
    if machine!=0x8664{bail!("Only AMD64 Windows worker binaries are allowed");}
    let section_count=u16le(data,coff+2)?;
    if !(1..=MAX_SECTIONS).contains(&section_count){
        bail!("Invalid Windows PE section count");
    }
    let optional_len=u16le(data,coff+16)?;
    let optional=coff.checked_add(20).context("PE optional header offset overflow")?;
    span(data,optional,optional_len)?;
    if optional_len<112+14*8 || u16le(data,optional)?!=0x20b{
        bail!("Expected PE32+ optional header with import directories");
    }
    let header_size=u32le(data,optional+60)?;
    let directories=u32le(data,optional+108)?;
    if header_size==0||header_size>data.len()||directories<14{
        bail!("Invalid PE header/data-directory metadata");
    }
    let section_table=optional.checked_add(optional_len)
        .context("PE section table offset overflow")?;
    let mut sections=Vec::with_capacity(section_count);
    for index in 0..section_count{
        let off=section_table.checked_add(index*40)
            .context("PE section-table offset overflow")?;
        span(data,off,40)?;
        let section=Section{
            virtual_size:u32le(data,off+8)?,
            rva:u32le(data,off+12)?,
            raw_size:u32le(data,off+16)?,
            raw_offset:u32le(data,off+20)?,
        };
        if section.raw_size>0{span(data,section.raw_offset,section.raw_size)?;}
        sections.push(section);
    }
    // Refuse overlapping virtual address ranges to avoid ambiguous RVA
    // resolution. Ignore zero-size uninitialized sections.
    sections.sort_by_key(|s|s.rva);
    for pair in sections.windows(2){
        let reach=pair[0].virtual_size.max(pair[0].raw_size);
        if reach>0 && pair[0].rva.checked_add(reach)
            .is_none_or(|end|end>pair[1].rva){
            bail!("Overlapping PE virtual sections");
        }
    }
    let directory=|id:usize|->Result<(usize,usize)>{
        let location=optional+112+id*8;
        if location+8>optional+optional_len{bail!("PE optional header truncates import directory");}
        Ok((u32le(data,location)?,u32le(data,location+4)?))
    };
    let (delay_rva,delay_size)=directory(13)?;
    if delay_rva!=0||delay_size!=0{
        bail!("Delayed DLL loading is not permitted for the fixed AppContainer Worker");
    }
    let (import_rva,import_size)=directory(1)?;
    if (import_rva==0)!=(import_size==0){
        bail!("Partially specified PE import directory");
    }
    let mut names=BTreeSet::new();
    let mut redistributables=BTreeSet::new();
    if import_rva!=0{
        if import_size<20||import_size>MAX_IMPORT_DIRECTORY{
            bail!("PE import descriptor table exceeds bounds");
        }
        let mut terminated=false;
        for index in 0..MAX_DESCRIPTORS{
            let within=index.checked_mul(20).context("PE import index overflow")?;
            if within+20>import_size{break;}
            let pos=rva_offset(data,&sections,header_size,import_rva+within,20)?;
            let raw=span(data,pos,20)?;
            if raw.iter().all(|&b|b==0){terminated=true;break;}
            let module_rva=u32le(data,pos+12)?;
            if module_rva==0{bail!("PE descriptor missing module name");}
            let module=imported_name(data,&sections,header_size,module_rva)?;
            if !system_module(&module)&&!msvc_runtime_module(&module){
                bail!("PE import {module:?} is not an approved Windows or MSVC runtime module");
            }
            if msvc_runtime_module(&module){redistributables.insert(module.clone());}
            names.insert(module);
        }
        if !terminated{bail!("Unterminated or oversized PE import table");}
    }
    Ok(PeImports{machine,modules:names.into_iter().collect(),msvc_runtime_modules:redistributables.into_iter().collect(),has_delay_load:false})
}

#[cfg(test)]
mod tests{
    use super::*;
    fn set16(data:&mut [u8],at:usize,value:u16){
        data[at..at+2].copy_from_slice(&value.to_le_bytes());
    }
    fn set32(data:&mut [u8],at:usize,value:u32){
        data[at..at+4].copy_from_slice(&value.to_le_bytes());
    }
    fn sample(dll:&[u8])->Vec<u8>{
        let mut d=vec![0u8;1024];
        d[0..2].copy_from_slice(b"MZ");
        set32(&mut d,0x3c,0x80);
        d[0x80..0x84].copy_from_slice(b"PE\0\0");
        set16(&mut d,0x84,0x8664);
        set16(&mut d,0x86,1);
        set16(&mut d,0x94,0xf0);
        let opt=0x98;
        set16(&mut d,opt,0x20b);
        set32(&mut d,opt+60,0x200);
        set32(&mut d,opt+108,16);
        set32(&mut d,opt+112+8,0x1100);
        set32(&mut d,opt+112+12,40);
        let sec=opt+0xf0;
        set32(&mut d,sec+8,0x200);
        set32(&mut d,sec+12,0x1000);
        set32(&mut d,sec+16,0x200);
        set32(&mut d,sec+20,0x200);
        set32(&mut d,0x300+12,0x1160);
        d[0x360..0x360+dll.len()].copy_from_slice(dll);
        d
    }
    #[test]
    fn known_windows_imports_are_inspected(){
        let good=sample(b"KERNEL32.dll\0");
        let imports=inspect_pe_imports(&good).unwrap();
        assert_eq!(imports.machine,0x8664);
        assert_eq!(imports.modules,vec!["kernel32.dll"]);
        assert!(!imports.has_delay_load);
        assert!(imports.msvc_runtime_modules.is_empty());
        let msvc=inspect_pe_imports(&sample(b"VCRUNTIME140.dll\0")).unwrap();
        assert_eq!(msvc.msvc_runtime_modules,vec!["vcruntime140.dll"]);
    }
    #[test]
    fn import_name_must_stay_inside_initialized_section(){
        let mut invalid=sample(b"KERNEL32.dll\0");
        // The import descriptor remains in the file-backed section, but the
        // DLL name crosses its raw boundary into adjacent file bytes.
        set32(&mut invalid,0x98+0xf0+16,0x140);
        set32(&mut invalid,0x300+12,0x113a);
        invalid[0x33a..0x33a+13].copy_from_slice(b"KERNEL32.dll\0");
        assert!(inspect_pe_imports(&invalid).is_err(),
            "Unmapped PE bytes must not complete an import name");

        let valid=sample(b"KERNEL32.dll\0");
        for len in 0..valid.len(){
            assert!(inspect_pe_imports(&valid[..len]).is_err(),
                "Truncated PE input {len} unexpectedly accepted");
        }
    }
    #[test]
    fn malformed_and_unknown_pe_imports_fail_closed(){
        for dll in [b"evil.dll\0".as_slice(),b"../path.dll\0",b"foo.exe\0",b"file:stream.dll\0"]{
            assert!(inspect_pe_imports(&sample(dll)).is_err());
        }
        let mut invalid=sample(b"KERNEL32.dll\0");
        invalid[0]=0;
        assert!(inspect_pe_imports(&invalid).is_err());
        let mut invalid=sample(b"KERNEL32.dll\0");
        set32(&mut invalid,0x3c,0xff_ff_ff_f0);
        assert!(inspect_pe_imports(&invalid).is_err());
        let mut invalid=sample(b"KERNEL32.dll\0");
        set32(&mut invalid,0x98+112+13*8,0x1200);
        set32(&mut invalid,0x98+112+13*8+4,32);
        assert!(inspect_pe_imports(&invalid).is_err());
        let mut invalid=sample(b"KERNEL32.dll\0");
        set32(&mut invalid,0x300+12,0x7fff_ff00);
        assert!(inspect_pe_imports(&invalid).is_err());
        assert!(inspect_pe_imports(b"").is_err());
        assert!(inspect_pe_imports(&vec![0u8;MAX_PE_BYTES+1]).is_err());
    }
}
