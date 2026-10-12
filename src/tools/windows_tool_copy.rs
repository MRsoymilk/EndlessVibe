//! Bounded per-job copies of selected native SDK/compiler leaf groups.
//!
//! Only the policy layer supplies source roots. This module never changes a
//! source ACL, follows a link, invokes a program, or copies a host home. The
//! If one selected leaf cannot have its DACL edited, the selected leaves of
//! that SDK/MSVC family travel together to preserve relative file references.
//! Returned pins survive until the private profile's tools tree is sealed.
use anyhow::{bail,Context,Result};
use std::{
    collections::{BTreeMap,BTreeSet},ffi::OsString,fs::{self,File,OpenOptions},
    io::{Read,Write},mem::zeroed,
    os::windows::{ffi::OsStringExt,fs::OpenOptionsExt,
        io::{AsRawHandle,FromRawHandle}},
    path::{Component,Path,PathBuf,Prefix},ptr,sync::Arc,time::Instant,
};
use windows_sys::Win32::{
    Foundation::DuplicateHandle,
    Storage::FileSystem::{
        GetFileInformationByHandle,GetFinalPathNameByHandleW,BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_DIRECTORY,FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS,FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ,FILE_SHARE_WRITE,
    },
    System::Threading::GetCurrentProcess,
};

const MAX_ROOTS:usize=64;
const MAX_NODES:usize=100_000;
const MAX_DEPTH:usize=64;
const MAX_FILE_BYTES:u64=512*1024*1024;
const MAX_TOTAL_BYTES:u64=8*1024*1024*1024;
const READ_CONTROL:u32=0x0002_0000;
const WRITE_DAC:u32=0x0004_0000;
const READ_ATTRIBUTES:u32=0x80;
const GENERIC_READ:u32=0x8000_0000;
const FILE_GENERIC_READ:u32=0x0012_0089;
const DIRECTORY_READ:u32=READ_CONTROL|READ_ATTRIBUTES|0x21;

#[link(name="kernel32")]
unsafe extern "system"{
    fn CompareStringOrdinal(a:*const u16,a_len:i32,b:*const u16,b_len:i32,ignore_case:i32)->i32;
}

pub(super) fn check_deadline(deadline:Instant)->Result<()>{
    if Instant::now()>=deadline{bail!("Windows sandbox preparation exceeded its 90-second deadline");}
    Ok(())
}
fn key(path:&Path)->Result<String>{
    let text=path.to_str().context("Native tool copy paths must be Unicode")?;
    Ok(text.strip_prefix(r"\\?\").unwrap_or(text).replace('/',"\\")
        .trim_end_matches('\\').to_ascii_lowercase())
}
fn equal_path(a:&Path,b:&Path)->Result<bool>{
    let a=key(a)?.encode_utf16().collect::<Vec<_>>();
    let b=key(b)?.encode_utf16().collect::<Vec<_>>();
    Ok(a.len()==b.len()&&unsafe{CompareStringOrdinal(
        a.as_ptr(),a.len() as i32,b.as_ptr(),b.len() as i32,1)}==2)
}
fn native_path(path:&Path)->Result<PathBuf>{
    let text=path.to_str().context("Native tool copy path is not Unicode")?;
    Ok(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text)))
}
fn final_path(file:&File)->Result<PathBuf>{
    let mut buffer=vec![0u16;1024];
    loop{
        let length=unsafe{GetFinalPathNameByHandleW(file.as_raw_handle() as _,
            buffer.as_mut_ptr(),buffer.len() as u32,0)} as usize;
        if length==0{return Err(std::io::Error::last_os_error()).context("Resolve pinned tool copy path");}
        if length<buffer.len(){return Ok(PathBuf::from(OsString::from_wide(&buffer[..length])));}
        if length>32760{bail!("Tool copy path exceeds Windows bounds");}
        buffer.resize(length+1,0);
    }
}
#[derive(Clone,Copy,Debug,Eq,Ord,PartialEq,PartialOrd)]
struct FileIdentity{volume:u32,index:u64}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Information{identity:FileIdentity,directory:bool,links:u32,bytes:u64}
fn information(file:&File)->Result<Information>{
    let mut info:BY_HANDLE_FILE_INFORMATION=unsafe{zeroed()};
    if unsafe{GetFileInformationByHandle(file.as_raw_handle() as _,&mut info)}==0{
        return Err(std::io::Error::last_os_error()).context("Read pinned tool copy identity");
    }
    if info.dwFileAttributes&FILE_ATTRIBUTE_REPARSE_POINT!=0{
        bail!("Native tool copies refuse reparse points and junctions");
    }
    let identity=FileIdentity{volume:info.dwVolumeSerialNumber,
        index:((info.nFileIndexHigh as u64)<<32)|info.nFileIndexLow as u64};
    if identity.volume==0||identity.index==0{bail!("Native tool copy needs a stable local file identity");}
    let directory=info.dwFileAttributes&FILE_ATTRIBUTE_DIRECTORY!=0;
    Ok(Information{identity,directory,links:info.nNumberOfLinks,
        bytes:if directory{0}else{((info.nFileSizeHigh as u64)<<32)|info.nFileSizeLow as u64}})
}
struct Pinned{file:File,path:PathBuf,info:Information}
impl Pinned{
    fn check(&self)->Result<()>{
        if information(&self.file)?!=self.info||!equal_path(&final_path(&self.file)?,&self.path)?{
            bail!("Tool copy object changed identity, size, path, type or link count");
        }
        Ok(())
    }
    fn dacl_editable(&self)->Result<bool>{
        self.check()?;
        let access=READ_CONTROL|WRITE_DAC|READ_ATTRIBUTES|if self.info.directory{0x20}else{0};
        let opened=OpenOptions::new().access_mode(access)
            .share_mode(FILE_SHARE_READ|FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT).open(&self.path);
        let file=match opened{
            Ok(file)=>file,
            Err(error)if error.raw_os_error()==Some(5)=>return Ok(false),
            Err(error)=>return Err(error).with_context(||format!(
                "Probe native tool ACL editing rights without changing permissions: {}",self.path.display())),
        };
        if information(&file)?!=self.info||!equal_path(&final_path(&file)?,&self.path)?{
            bail!("Native tool changed during its ACL editing probe");
        }
        Ok(true)
    }
}
#[derive(Default)]
struct Pins{entries:BTreeMap<String,Arc<Pinned>>}
impl Pins{
    fn insert(&mut self,file:File,expected:&Path)->Result<Arc<Pinned>>{
        let info=information(&file)?;
        let path=final_path(&file)?;
        if !equal_path(&path,expected)?{bail!("Tool copy object resolved outside its pinned ancestry");}
        let value=Arc::new(Pinned{file,path,info});
        self.entries.insert(key(expected)?,value.clone());
        Ok(value)
    }
    fn component(&mut self,path:&Path,directory:bool)->Result<Arc<Pinned>>{
        let lookup=key(path)?;
        if let Some(existing)=self.entries.get(&lookup){existing.check()?;return Ok(existing.clone());}
        if self.entries.len()>MAX_NODES+MAX_ROOTS*MAX_DEPTH{bail!("Tool copy exceeds its pin inventory bound");}
        // Ancestors remain traversable by the broker. Regular file pins do
        // not share write or delete access, so the copied bytes cannot change.
        let file=OpenOptions::new().access_mode(if directory{DIRECTORY_READ}else{READ_CONTROL|READ_ATTRIBUTES})
            .share_mode(if directory{FILE_SHARE_READ|FILE_SHARE_WRITE}else{FILE_SHARE_READ})
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path).with_context(||format!("Pin native tool copy object {}",path.display()))?;
        let value=self.insert(file,path)?;
        if value.info.directory!=directory{bail!("Tool copy path changed type during opening");}
        Ok(value)
    }
    fn open(&mut self,path:&Path)->Result<Arc<Pinned>>{
        let mut parts=path.components();
        let Some(Component::Prefix(prefix))=parts.next()else{bail!("Tool copies require absolute local drive paths");};
        let drive=match prefix.kind(){
            Prefix::Disk(letter)|Prefix::VerbatimDisk(letter)=>letter,
            _=>bail!("Native tool copies do not use network or device paths"),
        };
        if !matches!(parts.next(),Some(Component::RootDir)){bail!("Tool copy path is not absolute");}
        let mut current=PathBuf::from(format!(r"\\?\{}:\",(drive as char).to_ascii_uppercase()));
        let parts=parts.collect::<Vec<_>>();
        let mut parent=self.component(&current,true)?;
        for (index,part) in parts.iter().enumerate(){
            let Component::Normal(name)=part else{bail!("Tool copy paths cannot contain traversal");};
            let text=name.to_str().context("Tool copy path component is not Unicode")?;
            if text.is_empty()||text.contains(':')||text.ends_with([' ','.'])
                ||text.encode_utf16().count()>255{bail!("Tool copy path contains an ambiguous or oversized component");}
            if !parent.info.directory{bail!("Tool copy ancestor is not a directory");}
            current.push(name);
            let directory=if index+1<parts.len(){true}else{
                use std::os::windows::fs::MetadataExt;
                let metadata=fs::symlink_metadata(&current)
                    .with_context(||format!("Inspect native tool copy source {}",current.display()))?;
                if metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{
                    bail!("Native tool copies refuse reparse points");
                }
                metadata.is_dir()
            };
            parent=self.component(&current,directory)?;
        }
        Ok(parent)
    }
}

struct Tree{source:PathBuf,target_relative:PathBuf,nodes:Vec<Arc<Pinned>>,copy:bool}
fn inventory(sources:&[PathBuf],pins:&mut Pins,deadline:Instant)->Result<Vec<Tree>>{
    if sources.len()>MAX_ROOTS{bail!("Native tool copy accepts at most 64 exact SDK/tool roots");}
    let mut seen=BTreeSet::new();
    let mut trees=Vec::new();
    fn walk(path:&Path,depth:usize,pins:&mut Pins,tree:&mut Tree,
        seen:&mut BTreeSet<String>,deadline:Instant)->Result<()>{
        check_deadline(deadline)?;
        if depth>MAX_DEPTH||seen.len()>=MAX_NODES{bail!("Native tool inventory exceeds its depth or 100000-object bound");}
        let node=pins.open(path)?;
        if !seen.insert(key(&node.path)?){bail!("Native tool copy roots overlap or repeat a path");}
        if !tree.copy&&!node.dacl_editable()?{tree.copy=true;}
        tree.nodes.push(node.clone());
        if node.info.directory{
            for entry in fs::read_dir(&node.path).context("Enumerate a pinned native tool directory")?{
                let entry=entry?;
                walk(&node.path.join(entry.file_name()),depth+1,pins,tree,seen,deadline)?;
            }
        }
        Ok(())
    }
    for source in sources{
        check_deadline(deadline)?;
        let root=pins.open(source)?;
        if !root.info.directory||root.path.components().count()<=2{
            bail!("Automatic SDK/tool copy roots must be precise directories");
        }
        let mut tree=Tree{source:native_path(&root.path)?,
            target_relative:PathBuf::from(format!("root-{:02}",trees.len())),
            nodes:Vec::new(),copy:false};
        walk(&root.path,0,pins,&mut tree,&mut seen,deadline)?;
        trees.push(tree);
    }
    // A copied file may have multiple names only if every name is within the
    // authorized source inventory. No source hard link is reused at the target.
    let mut aliases=BTreeMap::<FileIdentity,(u32,usize)>::new();
    for node in trees.iter().flat_map(|tree|tree.nodes.iter()){
        check_deadline(deadline)?;
        node.check()?;
        if node.info.directory{continue;}
        let entry=aliases.entry(node.info.identity).or_insert((node.info.links,0));
        if entry.0!=node.info.links{bail!("Native tool link count changed during inventory");}
        entry.1+=1;
    }
    if aliases.values().any(|(links,names)|*links==0||*links as usize!=*names){
        bail!("Native tool copy refused a hard link outside its authorized source inventory");
    }
    Ok(trees)
}

/// Newly written files are retained through read-only, non-inheritable handles
/// which do not share write/delete access. Drop after seal_private_tools has
/// verified the complete tree, and always before profile cleanup.
pub(super) struct StagedTools{
    pub(super) mappings:Vec<(PathBuf,PathBuf)>,
    _targets:Pins,
}
fn readonly_duplicate(file:&File)->Result<File>{
    let mut raw=ptr::null_mut();
    if unsafe{DuplicateHandle(GetCurrentProcess(),file.as_raw_handle() as _,
        GetCurrentProcess(),&mut raw,FILE_GENERIC_READ,0,0)}==0{
        return Err(std::io::Error::last_os_error()).context("Retain a read-only private tool handle");
    }
    Ok(unsafe{File::from_raw_handle(raw as _)})
}
fn relative_parts(path:&Path)->Result<Vec<OsString>>{
    path.components().map(|part|match part{
        Component::Normal(name)if !name.is_empty()=>{
            let name=name.to_str().context("Private tool layout must be Unicode")?;
            if name.contains(':')||name.ends_with([' ','.'])||name.encode_utf16().count()>255{
                bail!("Private tool layout contains an ambiguous component");
            }
            Ok(OsString::from(name))
        },
        _=>bail!("Private tool layout must contain only relative directory names"),
    }).collect()
}
fn reserve_target(pins:&Pins)->Result<()>{
    if pins.entries.len()>=MAX_NODES{bail!("Private native tools exceed the 100000-object bound");}
    Ok(())
}
fn create_layout_parents(private:&Path,relative:&Path,pins:&mut Pins,deadline:Instant)->Result<()>{
    let mut current=private.to_owned();
    for part in relative_parts(relative)?{
        check_deadline(deadline)?;
        current.push(part);
        if let Some(existing)=pins.entries.get(&key(&current)?){
            existing.check()?;
            if !existing.info.directory{bail!("Private tool layout parent is not a directory");}
        }else{
            reserve_target(pins)?;
            fs::create_dir(&current).context("Create a new private SDK/tool layout directory")?;
            pins.open(&current)?;
        }
    }
    Ok(())
}
fn copy_trees(trees:&[Tree],private_tools:&Path,deadline:Instant,
    file_limit:u64,total_limit:u64)->Result<StagedTools>{
    let mut total=0u64;let mut files=0usize;
    for node in trees.iter().filter(|tree|tree.copy).flat_map(|tree|tree.nodes.iter()){
        check_deadline(deadline)?;
        if node.info.directory{continue;}
        if node.info.bytes>file_limit{bail!("Native tool copy file exceeds the 512 MiB limit: {}",node.path.display());}
        total=total.checked_add(node.info.bytes).context("Native tool copy byte count overflow")?;
        if total>total_limit{bail!("Native tool copies exceed the 8 GiB per-job byte limit");}
        files+=1;
    }
    let mut staged=StagedTools{mappings:Vec::new(),_targets:Pins::default()};
    if !trees.iter().any(|tree|tree.copy){return Ok(staged);}
    let parent=private_tools.parent().context("Private tools directory has no profile parent")?;
    let parent=staged._targets.open(parent)?;
    if !parent.info.directory{bail!("Private tools profile parent is not a directory");}
    check_deadline(deadline)?;
    reserve_target(&staged._targets)?;
    fs::create_dir(private_tools).context("Create a new per-job private tools directory")?;
    let private=staged._targets.open(private_tools)?;
    for tree in trees.iter().filter(|tree|tree.copy){
        check_deadline(deadline)?;
        let parts=relative_parts(&tree.target_relative)?;
        if parts.is_empty(){bail!("Private tool root cannot replace the tools directory");}
        create_layout_parents(&private.path,
            tree.target_relative.parent().unwrap_or_else(||Path::new("")),&mut staged._targets,deadline)?;
        let target=private.path.join(&tree.target_relative);
        for node in &tree.nodes{
            check_deadline(deadline)?;
            node.check()?;
            let relative=node.path.strip_prefix(&tree.nodes[0].path)
                .context("Native tool source escaped its authorized root")?;
            let destination=target.join(relative);
            reserve_target(&staged._targets)?;
            if node.info.directory{
                fs::create_dir(&destination).with_context(||format!("Create new private tool directory {}",destination.display()))?;
                staged._targets.open(&destination)?;
                continue;
            }
            let mut input=OpenOptions::new().access_mode(GENERIC_READ).share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(&node.path)
                .with_context(||format!("Read pinned native tool {}",node.path.display()))?;
            if information(&input)?!=node.info||!equal_path(&final_path(&input)?,&node.path)?{
                bail!("Native tool source changed before copying");
            }
            let mut output=OpenOptions::new().read(true).write(true).create_new(true)
                .share_mode(FILE_SHARE_READ).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&destination).context("Create a new private tool file without sharing source links")?;
            let mut buffer=[0u8;65536];let mut copied=0u64;
            loop{
                check_deadline(deadline)?;
                let size=input.read(&mut buffer)?;if size==0{break;}
                copied=copied.checked_add(size as u64).context("Native tool copy size overflow")?;
                if copied>node.info.bytes||copied>file_limit{bail!("Native tool grew beyond its inspected copy budget");}
                output.write_all(&buffer[..size])?;
            }
            if copied!=node.info.bytes{bail!("Native tool size changed during copying");}
            output.flush()?;
            let pinned=readonly_duplicate(&output)?;
            drop(output);
            let copied=staged._targets.insert(pinned,&destination)?;
            if copied.info.directory||copied.info.links!=1||copied.info.bytes!=node.info.bytes{
                bail!("Private native tool is not a new independent regular file");
            }
            node.check()?;
        }
        staged.mappings.push((tree.source.clone(),native_path(&target)?));
    }
    for target in staged._targets.entries.values(){check_deadline(deadline)?;target.check()?;}
    check_deadline(deadline)?;
    tracing::info!(roots=staged.mappings.len(),files,bytes=total,
        "Prepared bounded private native tool copies; source permissions were unchanged");
    Ok(staged)
}

fn close_copy_families(trees:&mut [Tree],layouts:&[(PathBuf,PathBuf)],
    excluded_roots:&[PathBuf])->Result<()>{
    if layouts.len()>2{bail!("Private tool copying supports only the selected SDK and MSVC layouts");}
    let mut families=Vec::with_capacity(trees.len());
    let mut copy_families=BTreeSet::new();
    for tree in trees.iter_mut(){
        let (base,namespace)=layouts.iter().filter(|(base,_)|tree.source.starts_with(base))
            .max_by_key(|(base,_)|base.components().count())
            .context("Automatic tool copy root has no selected SDK/MSVC installation layout")?;
        if !matches!(namespace.to_str(),Some("sdk"|"msvc")){
            bail!("Private tool layout must use its fixed SDK or MSVC namespace");
        }
        tree.target_relative=namespace.join(tree.source.strip_prefix(base)?);
        relative_parts(&tree.target_relative)?;
        if tree.copy{copy_families.insert(namespace.clone());}
        families.push(namespace.clone());
    }
    // An explicit source=target mount cannot silently move. Refuse a partial
    // private SDK instead of leaving cross-leaf relative paths unresolved.
    for (tree,family) in trees.iter().zip(&families){
        if copy_families.contains(family)&&excluded_roots.iter().any(|root|root==&tree.source){
            bail!("Cannot relocate the selected {} toolchain: an explicit readonly_mounts source=target fixes the automatic leaf {} at its host path; remove the overlapping manual mount to use private tool copies",
                family.display(),tree.source.display());
        }
    }
    for (tree,family) in trees.iter_mut().zip(families){
        tree.copy=copy_families.contains(&family);
    }
    Ok(())
}

/// Probe only exact automatically discovered SDK/MSVC leaves. If any member
/// has a real access-denied result for WRITE_DAC, copy the selected leaves of
/// that family together. Other probe failures remain errors; no installation
/// parent, additional version/architecture, or manual mount is copied.
pub(super) fn stage_uneditable_roots(sources:&[PathBuf],layouts:&[(PathBuf,PathBuf)],
    excluded_roots:&[PathBuf],private_tools:&Path,deadline:Instant)->Result<StagedTools>{
    check_deadline(deadline)?;
    let mut sources_pins=Pins::default();
    let mut trees=inventory(sources,&mut sources_pins,deadline)?;
    close_copy_families(&mut trees,layouts,excluded_roots)?;
    copy_trees(&trees,private_tools,deadline,MAX_FILE_BYTES,MAX_TOTAL_BYTES)
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::time::Duration;
    fn deadline()->Instant{Instant::now()+Duration::from_secs(90)}
    #[test]
    fn editable_native_tools_stay_in_place_without_creating_private_copies(){
        let temporary=tempfile::tempdir().unwrap();
        let source=temporary.path().join("source");fs::create_dir(&source).unwrap();
        fs::write(source.join("header.h"),b"native header").unwrap();
        let target=temporary.path().join("tools");
        let staged=stage_uneditable_roots(&[source.clone()],
            &[(source,PathBuf::from("msvc"))],&[],&target,deadline()).unwrap();
        assert!(staged.mappings.is_empty());assert!(!target.exists());
    }
    #[test]
    fn private_copy_is_independent_bounded_and_holds_immutable_target_pins(){
        let temporary=tempfile::tempdir().unwrap();
        let source=temporary.path().join("source");fs::create_dir(&source).unwrap();
        let original=source.join("header.h");fs::write(&original,b"native header").unwrap();
        let mut pins=Pins::default();let mut trees=inventory(&[source.clone()],&mut pins,deadline()).unwrap();
        trees[0].copy=true;
        let rejected=temporary.path().join("too-large");
        assert!(copy_trees(&trees,&rejected,deadline(),4,1024).is_err());
        assert!(!rejected.exists());
        let target=temporary.path().join("tools");
        let staged=copy_trees(&trees,&target,deadline(),1024,1024).unwrap();
        let copied=staged.mappings[0].1.join("header.h");
        assert_eq!(fs::read(&copied).unwrap(),b"native header");
        assert!(fs::write(&copied,b"change").is_err());
        assert!(fs::remove_file(&copied).is_err());
        let copied_identity=information(&File::open(&copied).unwrap()).unwrap().identity;
        assert_ne!(copied_identity,information(&File::open(&original).unwrap()).unwrap().identity);
        assert!(copy_trees(&trees,&target,deadline(),1024,1024).is_err());
        drop(staged);
        fs::write(&copied,b"private change").unwrap();
        assert_eq!(fs::read(&original).unwrap(),b"native header");
    }
    #[test]
    fn source_inventory_rejects_external_hardlinks_and_expired_preparation(){
        let temporary=tempfile::tempdir().unwrap();
        let source=temporary.path().join("source");fs::create_dir(&source).unwrap();
        let outside=temporary.path().join("outside");fs::write(&outside,b"external bytes").unwrap();
        fs::hard_link(&outside,source.join("header.h")).unwrap();
        let target=temporary.path().join("tools");
        let layouts=[(source.clone(),PathBuf::from("msvc"))];
        let error=stage_uneditable_roots(&[source.clone()],&layouts,&[],&target,deadline()).err().unwrap();
        assert!(format!("{error:#}").contains("hard link"),"{error:#}");
        assert!(!target.exists());
        let error=stage_uneditable_roots(&[source],&layouts,&[],&target,Instant::now()).err().unwrap();
        assert!(format!("{error:#}").contains("deadline"),"{error:#}");
        assert!(!target.exists());
    }
    #[test]
    fn private_sdk_copy_closes_its_selected_family_without_copying_other_tools_or_the_installation(){
        let temporary=tempfile::tempdir().unwrap();
        let sdk=temporary.path().join("sdk");
        let shared=sdk.join("Include/version/shared");
        let um=sdk.join("Include/version/um");
        fs::create_dir_all(&shared).unwrap();fs::create_dir_all(&um).unwrap();
        fs::write(shared.join("base.h"),b"selected shared header").unwrap();
        fs::write(um.join("api.h"),b"#include \"../shared/base.h\"").unwrap();
        fs::write(sdk.join("not-selected.txt"),b"must not copy this installation-level file").unwrap();
        let msvc=temporary.path().join("msvc");
        let bin=msvc.join("bin");fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("cl.exe"),b"an editable compiler stays in place").unwrap();
        let mut pins=Pins::default();
        let mut trees=inventory(&[shared,um,bin],&mut pins,deadline()).unwrap();
        let sdk_native=native_path(&fs::canonicalize(&sdk).unwrap()).unwrap();
        let msvc_native=native_path(&fs::canonicalize(&msvc).unwrap()).unwrap();
        assert!(trees.iter().all(|tree|!tree.copy));
        // Isolate the family planner's input: one SDK leaf has been selected
        // by a WRITE_DAC access-denied probe; its sibling and MSVC have not.
        trees[1].copy=true;
        let layouts=[(sdk_native,PathBuf::from("sdk")),(msvc_native,PathBuf::from("msvc"))];
        let excluded=vec![trees[0].source.clone()];
        let error=close_copy_families(&mut trees,&layouts,&excluded).unwrap_err();
        assert!(error.to_string().contains("explicit readonly_mounts"));
        assert!(!trees[0].copy&&trees[1].copy&&!trees[2].copy);
        close_copy_families(&mut trees,&layouts,&[]).unwrap();
        assert!(trees[0].copy&&trees[1].copy&&!trees[2].copy);
        let target=temporary.path().join("tools");
        let staged=copy_trees(&trees,&target,deadline(),1024,4096).unwrap();
        assert_eq!(fs::read(target.join("sdk/Include/version/um/../shared/base.h")).unwrap(),
            b"selected shared header");
        assert!(!target.join("sdk/not-selected.txt").exists());
        assert!(!target.join("msvc").exists());
        assert_eq!(staged.mappings.len(),2);
    }
}
