//! Resolve configured native tools without exposing the service account's home.
//!
//! Rustup proxies are replaced with already-installed compiler binaries. Cargo
//! gets a fresh home with only its locked, cached public crates.io packages.
//! Host Cargo configuration and registry credentials are never copied.
use crate::{config::Config,security::paths::Root};
use anyhow::{bail,Context,Result};
use sha2::{Digest,Sha256};
use std::{
    collections::{BTreeMap,BTreeSet},env,ffi::OsString,fs,
    io::Read,os::windows::fs::MetadataExt,
    path::{Component,Path,PathBuf,Prefix},
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

const MAX_SETTINGS_BYTES:u64=1024*1024;
const MAX_PROXY_BYTES:u64=128*1024*1024;
const MAX_LOCK_PACKAGES:usize=4096;
const MAX_CARGO_CACHE_FILE_BYTES:usize=64*1024*1024;
const MAX_CARGO_CACHE_TOTAL_BYTES:usize=512*1024*1024;
const MAX_SPARSE_NAMESPACES:usize=8;
const RUST_PROGRAMS:&[&str]=&["cargo","rustc","rustdoc","rustfmt","cargo-fmt",
    "cargo-clippy","clippy-driver"];

#[derive(Clone,Debug)]
pub(super) struct Toolchains{
    pub(super) programs:BTreeMap<String,PathBuf>,
    pub(super) readonly_roots:Vec<PathBuf>,
    pub(super) path_entries:Vec<PathBuf>,
    pub(super) environment:BTreeMap<OsString,OsString>,
    cargo_home:Option<PathBuf>,
    automatic_roots:Vec<PathBuf>,
    copy_exclusions:Vec<PathBuf>,
    copy_layouts:Vec<(PathBuf,PathBuf)>,
}
#[derive(Clone,Debug,Default)]
struct Homes{
    user:Option<PathBuf>,
    cargo:Option<PathBuf>,
    rustup:Option<PathBuf>,
    toolchain:Option<String>,
}
impl Homes{
    fn current()->Self{
        let user=env::var_os("USERPROFILE").or_else(||env::var_os("HOME")).map(PathBuf::from);
        let cargo=env::var_os("CARGO_HOME").map(PathBuf::from)
            .or_else(||user.as_ref().map(|path|path.join(".cargo")));
        let rustup=env::var_os("RUSTUP_HOME").map(PathBuf::from)
            .or_else(||user.as_ref().map(|path|path.join(".rustup")));
        Self{user,cargo,rustup,toolchain:env::var("RUSTUP_TOOLCHAIN").ok()}
    }
}
fn simple_program(name:&str)->bool{
    !name.is_empty()&&name.len()<=64&&name.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b))
}
fn local_absolute(path:&Path)->Result<()>{
    if !path.is_absolute()||!matches!(path.components().next(),
        Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)|Prefix::VerbatimDisk(_))){
        bail!("Windows toolchain paths must be absolute paths on a local drive");
    }
    if path.components().any(|part|matches!(part,Component::ParentDir)){
        bail!("Windows toolchain paths cannot contain parent traversal");
    }
    Ok(())
}
fn no_reparse(path:&Path)->Result<()>{
    local_absolute(path)?;
    let mut current=PathBuf::new();
    for part in path.components(){
        current.push(part.as_os_str());
        if matches!(part,Component::Prefix(_)){continue;}
        let metadata=fs::symlink_metadata(&current)
            .with_context(||format!("Inspect toolchain path {}",current.display()))?;
        if metadata.file_attributes()&FILE_ATTRIBUTE_REPARSE_POINT!=0{
            bail!("Windows toolchain path contains a reparse point: {}",current.display());
        }
    }
    Ok(())
}
fn native_path(path:&Path)->Result<PathBuf>{
    no_reparse(path)?;
    canonical_local(path)
}
fn canonical_local(path:&Path)->Result<PathBuf>{
    let canonical=fs::canonicalize(path).context("Canonicalize native toolchain path")?;
    local_absolute(&canonical)?;
    // Rust/Win32 can open verbatim paths, but compiler command-line parsers do
    // not all accept the prefix in LIB/INCLUDE and executable settings.
    let text=canonical.to_str().context("Native toolchain path is not valid Unicode")?;
    Ok(PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(text)))
}
fn configured_executable(path:&str,program:&str)->Result<Option<PathBuf>>{
    if !simple_program(program){bail!("Windows tool names must be simple executable names");}
    for root in env::split_paths(path){
        local_absolute(&root)?;
        let candidate=root.join(format!("{program}.exe"));
        if candidate.is_file(){return native_path(&candidate).map(Some);}
    }
    Ok(None)
}
fn digest_file(path:&Path)->Result<Vec<u8>>{
    let mut file=fs::File::open(path).context("Open configured Rust executable")?;
    if file.metadata()?.len()>MAX_PROXY_BYTES{bail!("Rust proxy executable exceeds inspection limit");}
    let mut hash=Sha256::new();let mut bytes=[0u8;65536];let mut total=0u64;
    loop{
        let size=file.read(&mut bytes)?;if size==0{break;}
        total=total.saturating_add(size as u64);
        if total>MAX_PROXY_BYTES{bail!("Rust proxy executable grew during inspection");}
        hash.update(&bytes[..size]);
    }
    Ok(hash.finalize().to_vec())
}
fn rustup_proxy(program:&Path)->Result<bool>{
    let rustup=program.parent().context("Rust executable has no parent")?.join("rustup.exe");
    if !rustup.is_file(){return Ok(false);}
    let rustup=native_path(&rustup)?;
    if fs::metadata(program)?.len()!=fs::metadata(&rustup)?.len(){return Ok(false);}
    Ok(digest_file(program)?==digest_file(&rustup)?)
}
fn read_small(path:&Path)->Result<String>{
    no_reparse(path)?;
    let mut bytes=Vec::new();fs::File::open(path)?.take(MAX_SETTINGS_BYTES+1).read_to_end(&mut bytes)?;
    if bytes.len() as u64>MAX_SETTINGS_BYTES{bail!("Toolchain settings exceed inspection limit");}
    String::from_utf8(bytes).context("Toolchain settings are not valid UTF-8")
}
fn valid_toolchain_name(name:&str)->bool{
    !name.is_empty()&&name.len()<=128&&name.as_bytes()[0].is_ascii_alphanumeric()
        &&name.bytes().all(|b|b.is_ascii_alphanumeric()||b"_.-".contains(&b))
}
fn installed_rust_toolchain(homes:&Homes)->Result<Option<PathBuf>>{
    let Some(home)=&homes.rustup else{return Ok(None)};
    if !home.exists(){return Ok(None);}
    let home=native_path(home)?;
    let settings=home.join("settings.toml");
    let default=if settings.is_file(){
        let value:toml::Value=toml::from_str(&read_small(&settings)?)
            .context("Parse Rustup settings without invoking a proxy")?;
        match value.get("default_toolchain"){
            Some(value)=>Some(value.as_str().context("Rustup default_toolchain must be a string")?.to_owned()),
            None=>None,
        }
    }else{None};
    let Some(name)=homes.toolchain.as_deref().or(default.as_deref())else{return Ok(None)};
    if !valid_toolchain_name(name){
        bail!("Rustup toolchain selection must name an installed toolchain; configure a direct native PATH for custom paths");
    }
    for selected in [name.to_owned(),format!("{name}-x86_64-pc-windows-msvc")]{
        let candidate=home.join("toolchains").join(selected);
        if candidate.join("bin/rustc.exe").is_file(){return native_path(&candidate).map(Some);}
    }
    Ok(None)
}
fn rust_layout(program:&Path)->Option<PathBuf>{
    let bin=program.parent()?;
    if !bin.file_name()?.eq_ignore_ascii_case("bin"){return None;}
    let root=bin.parent()?;
    root.join("lib/rustlib").is_dir().then(||root.to_owned())
}
fn recognized_sparse_crates_io(name:&str)->bool{
    name.strip_prefix("index.crates.io-").is_some_and(|suffix|
        suffix.len()==16&&suffix.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)))
}
#[derive(Clone,Debug,Eq,Ord,PartialEq,PartialOrd)]
struct LockedCrate{name:String,version:String,checksum:String}
fn crates_io_locked_packages(project:&Path)->Result<Vec<LockedCrate>>{
    no_reparse(project)?;
    let project=Root::open(project).context("Open project for offline Cargo lockfile")?;
    let Some(bytes)=project.read_optional("Cargo.lock",MAX_SETTINGS_BYTES as usize)
        .context("Read project Cargo.lock without following links")?else{return Ok(Vec::new())};
    let text=std::str::from_utf8(&bytes).context("Cargo.lock is not valid UTF-8")?;
    let lock:toml::Value=toml::from_str(text).context("Parse Cargo.lock for offline package preparation")?;
    let Some(packages)=lock.get("package")else{return Ok(Vec::new())};
    let packages=packages.as_array().context("Cargo.lock package must be an array")?;
    if packages.len()>MAX_LOCK_PACKAGES{bail!("Cargo.lock exceeds the offline package limit");}
    let mut selected=BTreeMap::new();
    for package in packages{
        let Some(source)=package.get("source")else{continue};
        let source=source.as_str().context("Cargo.lock source must be a string")?;
        if !matches!(source,"registry+https://github.com/rust-lang/crates.io-index"
            |"registry+sparse+https://index.crates.io/"){
            bail!("Offline AppContainer Cargo supports cached crates.io dependencies and project-local paths; a lockfile dependency requires another registry or Git source");
        }
        let field=|key:&str|->Result<&str>{package.get(key).and_then(toml::Value::as_str)
            .with_context(||format!("Cargo.lock registry package needs a string {key}"))};
        let name=field("name")?;let version=field("version")?;let checksum=field("checksum")?;
        if name.is_empty()||name.len()>64||!name.as_bytes()[0].is_ascii_alphanumeric()
            ||!name.bytes().all(|byte|byte.is_ascii_alphanumeric()||b"_-".contains(&byte)){
            bail!("Cargo.lock contains an unsafe crate name");
        }
        if version.is_empty()||version.len()>128||!version.as_bytes()[0].is_ascii_digit()
            ||!version.bytes().all(|byte|byte.is_ascii_alphanumeric()||b".+-".contains(&byte)){
            bail!("Cargo.lock contains an unsafe crate version");
        }
        if checksum.len()!=64||!checksum.bytes().all(|byte|byte.is_ascii_hexdigit()){
            bail!("Cargo.lock registry packages require their original SHA-256 checksums");
        }
        let key=(name.to_owned(),version.to_owned());
        let checksum=checksum.to_ascii_lowercase();
        if selected.insert(key,checksum.clone()).is_some_and(|previous|previous!=checksum){
            bail!("Cargo.lock contains conflicting checksums for one registry package");
        }
    }
    Ok(selected.into_iter().map(|((name,version),checksum)|LockedCrate{name,version,checksum}).collect())
}
fn index_entry(name:&str)->String{
    let name=name.to_ascii_lowercase();
    match name.len(){
        1=>format!("1/{name}"),2=>format!("2/{name}"),3=>format!("3/{}/{name}",&name[..1]),
        _=>format!("{}/{}/{name}",&name[..2],&name[2..4]),
    }
}
fn sparse_caches(home:&Root)->Result<Vec<String>>{
    let entries=home.entries("registry/index").context("Offline Cargo needs an existing public crates.io sparse index cache")?;
    let mut names=entries.into_iter().filter(|entry|entry.kind=="directory"&&recognized_sparse_crates_io(&entry.name))
        .map(|entry|entry.name).collect::<Vec<_>>();
    // Cargo's internal URL hash has changed between releases. Preserve the
    // public namespaces already present on this machine instead of guessing
    // the selected compiler's hash or probing Cargo outside the sandbox.
    names.sort();names.dedup();
    if names.len()>MAX_SPARSE_NAMESPACES{bail!("Offline Cargo cache has too many public sparse namespaces");}
    Ok(names)
}
fn normalized_index_configuration(bytes:&[u8])->Result<Vec<u8>>{
    let value:serde_json::Value=serde_json::from_slice(bytes).context("Parse cached public crates.io index configuration")?;
    if value.get("auth-required").is_some_and(|flag|flag.as_bool()!=Some(false)){
        bail!("Public crates.io cache cannot require registry credentials");
    }
    let dl=value.get("dl").and_then(serde_json::Value::as_str)
        .context("Public crates.io index has no download URL")?;
    let public_url=|text:&str,host:&str|->bool{
        url::Url::parse(text).is_ok_and(|url|url.scheme()=="https"&&url.host_str()==Some(host)
            &&url.username().is_empty()&&url.password().is_none()&&url.query().is_none()
            &&url.fragment().is_none()&&url.port_or_known_default()==Some(443))
    };
    if !public_url(dl,"static.crates.io"){
        bail!("Cached crates.io index does not use the public download endpoint");
    }
    let mut clean=serde_json::Map::new();clean.insert("dl".into(),dl.into());
    if let Some(api)=value.get("api").filter(|value|!value.is_null()){
        let api=api.as_str().context("Cached crates.io API URL must be a string")?;
        if !public_url(api,"crates.io"){bail!("Cached crates.io index does not use the public API endpoint");}
        clean.insert("api".into(),api.into());
    }
    // Copy only the documented public fields. Never propagate authentication
    // fields, alternate service endpoints, or arbitrary host metadata.
    Ok(serde_json::to_vec(&serde_json::Value::Object(clean))?)
}
fn verify_sparse_index(bytes:&[u8],packages:&[LockedCrate],name:&str)->Result<usize>{
    // This format is private to Cargo. Reject an unrecognized format instead
    // of fabricating metadata; Cargo remains responsible for interpreting it.
    if bytes.len()<6||bytes[0]!=3{bail!("Cached Cargo sparse index format is unsupported");}
    let expected=packages.iter().filter(|package|package.name.eq_ignore_ascii_case(name))
        .map(|package|(package.version.as_str(),package.checksum.as_str())).collect::<BTreeMap<_,_>>();
    let mut available=BTreeSet::new();
    for part in bytes.split(|byte|*byte==0).filter(|part|part.first()==Some(&b'{')){
        let Ok(entry)=serde_json::from_slice::<serde_json::Value>(part)else{continue};
        if !entry["name"].as_str().is_some_and(|value|value.eq_ignore_ascii_case(name)){continue;}
        let Some(version)=entry["vers"].as_str()else{continue};
        if let Some(checksum)=expected.get(version){
            if entry["cksum"].as_str()!=Some(*checksum){
                bail!("Cached crates.io index contradicts Cargo.lock's checksum for {name} {version}");
            }
            available.insert(version.to_owned());
        }
    }
    // An absent version may belong to another target. Keep the original
    // bytes and let real Cargo --offline --locked resolve the target graph.
    Ok(available.len())
}
fn missing_file(error:&anyhow::Error)->bool{
    error.chain().any(|cause|cause.downcast_ref::<std::io::Error>()
        .is_some_and(|error|error.kind()==std::io::ErrorKind::NotFound))
}
fn optional_cache_bytes(home:&Root,relative:&str,limit:usize)->Result<Option<Vec<u8>>>{
    home.read_optional(relative,limit)
        .with_context(||format!("Read available offline Cargo cache file {relative}"))
}
fn copy_private_cache_bytes(private:&Root,relative:&str,bytes:&[u8],total:&mut usize)->Result<()>{
    *total=total.checked_add(bytes.len()).context("Offline Cargo cache byte count overflow")?;
    if *total>MAX_CARGO_CACHE_TOTAL_BYTES{bail!("Offline Cargo cache exceeds its 512 MiB per-job limit");}
    private.replace(relative,bytes,true,true).context("Create a new private Cargo cache file")
}
fn push_unique(values:&mut Vec<PathBuf>,path:PathBuf){if !values.contains(&path){values.push(path);}}
fn add_root(result:&mut Toolchains,path:&Path)->Result<()>{
    push_unique(&mut result.readonly_roots,native_path(path)?);Ok(())
}
fn prohibit_broad_root(root:&Path,homes:&Homes,state:&Path)->Result<()>{
    for protected in homes.user.iter().chain(homes.cargo.iter()).chain(homes.rustup.iter())
        .map(PathBuf::as_path).chain(std::iter::once(state)){
        if protected.exists(){
            // This is only a deny comparison, not a grant. An unrelated home
            // junction must not prevent discovery of a non-Rust executable.
            let protected=canonical_local(protected)?;
            if protected.starts_with(root){
                bail!("Read-only toolchain roots cannot expose a home, credential root, or service state: {}",root.display());
            }
        }
    }
    Ok(())
}
fn discover_with(config:&Config,programs:&[String],homes:&Homes,
    msvc:Option<crate::tools::windows_msvc::Toolchain>)->Result<Toolchains>{
    let mut result=Toolchains{programs:BTreeMap::new(),readonly_roots:Vec::new(),
        path_entries:Vec::new(),environment:BTreeMap::new(),cargo_home:None,
        automatic_roots:Vec::new(),copy_exclusions:Vec::new(),copy_layouts:Vec::new()};
    let mut selected_rust:Option<Option<PathBuf>>=None;
    for program in programs{
        if result.programs.contains_key(program){continue;}
        let Some(mut executable)=configured_executable(&config.execution.path,program)?else{continue};
        if RUST_PROGRAMS.contains(&program.as_str())&&rustup_proxy(&executable)?{
            if !config.execution.auto_discover_toolchains{
                bail!("Rustup proxy needs auto_discover_toolchains or a direct installed-toolchain PATH");
            }
            let selected=match &selected_rust{
                Some(selected)=>selected.clone(),
                None=>{let selected=installed_rust_toolchain(homes)?;selected_rust=Some(selected.clone());selected},
            };
            let Some(root)=selected else{continue};
            let direct=root.join("bin").join(format!("{program}.exe"));
            if !direct.is_file(){continue;}
            executable=native_path(&direct)?;
        }
        push_unique(&mut result.path_entries,executable.parent().unwrap().to_owned());
        if RUST_PROGRAMS.contains(&program.as_str()){
            if let Some(root)=rust_layout(&executable){
                add_root(&mut result,&root.join("bin"))?;
                add_root(&mut result,&root.join("lib"))?;
            }else{add_root(&mut result,&executable)?;}
        }else{
            // A configured executable does not authorize all sibling files.
            // Native dependency directories require explicit read-only roots.
            add_root(&mut result,&executable)?;
        }
        result.programs.insert(program.clone(),executable);
    }
    let rust_requested=RUST_PROGRAMS.iter().any(|name|result.programs.contains_key(*name));
    if rust_requested{
        for (program,key) in [("rustc","RUSTC"),("rustdoc","RUSTDOC")]{
            if let Some(path)=result.programs.get(program){
                result.environment.insert(key.into(),path.as_os_str().to_owned());
            }else if let Some(root)=result.programs.values().find_map(|path|rust_layout(path)){
                let path=root.join("bin").join(format!("{program}.exe"));
                if path.is_file(){
                    let path=native_path(&path)?;
                    result.environment.insert(key.into(),path.as_os_str().to_owned());
                    result.programs.insert(program.into(),path);
                }
            }
        }
        result.environment.insert("CARGO_NET_OFFLINE".into(),"true".into());
        if config.execution.auto_discover_toolchains&&result.programs.contains_key("cargo"){
            // Preparation copies only explicitly locked public packages into a
            // new private home. No access to the host Cargo home is granted.
            result.cargo_home=homes.cargo.clone();
        }
    }
    let compiler_requested=rust_requested||programs.iter().any(|program|
        matches!(program.as_str(),"cmake"|"ninja"|"cl"|"link"|"lib"));
    if compiler_requested&&config.execution.auto_discover_toolchains{
        if let Some(msvc)=msvc{
            for (name,path) in [("link",&msvc.linker),("cl",&msvc.compiler),("lib",&msvc.librarian)]{
                result.programs.insert(name.into(),native_path(path)?);
            }
            for root in msvc.readonly_roots(){
                let root=native_path(&root)?;
                push_unique(&mut result.readonly_roots,root.clone());
                push_unique(&mut result.automatic_roots,root);
            }
            // Prefer the selected MSVC tools over similarly named unrelated
            // utilities in configured PATH entries.
            let mut paths=msvc.path_entries().into_iter().map(|path|native_path(&path))
                .collect::<Result<Vec<_>>>()?;
            for entry in result.path_entries{push_unique(&mut paths,entry);}
            result.path_entries=paths;
            let mut environment=msvc.environment()?;
            // Canonicalize compiler paths before leaf remapping, including
            // case and slash aliases from Windows installation variables.
            for (key,value) in &mut environment{
                if matches!(key.to_str(),Some("LIB"|"LIBPATH"|"INCLUDE")){
                    *value=env::join_paths(env::split_paths(&*value)
                        .map(|path|native_path(&path)).collect::<Result<Vec<_>>>()?)?;
                }else if Path::new(&*value).is_absolute(){
                    *value=native_path(Path::new(&*value))?.into_os_string();
                }
            }
            for (name,namespace) in [("VCToolsInstallDir","msvc"),("WindowsSdkDir","sdk")]{
                let root=environment.get(std::ffi::OsStr::new(name))
                    .context("Selected native compiler has no installation layout")?;
                result.copy_layouts.push((native_path(Path::new(root))?,PathBuf::from(namespace)));
            }
            result.environment.extend(environment);
        }
    }
    for mount in &config.execution.readonly_mounts{
        if mount.source!=mount.target{
            bail!("Windows read-only roots must use the same native source and target path");
        }
        add_root(&mut result,&mount.source)?;
        let explicit=native_path(&mount.source)?;
        // An explicit native source=target mount keeps its requested path.
        // Only automatic SDK/MSVC roots may be replaced with private copies.
        for root in &result.automatic_roots{
            if root.starts_with(&explicit)||explicit.starts_with(root){
                push_unique(&mut result.copy_exclusions,root.clone());
            }
        }
    }
    for root in &result.readonly_roots{prohibit_broad_root(root,homes,&config.security.data_dir)?;}
    result.readonly_roots.sort();result.readonly_roots.dedup();
    result.automatic_roots.sort();result.automatic_roots.dedup();
    Ok(result)
}
/// Missing tools are omitted so the existing command preflight can produce
/// its structured JOB_PREFLIGHT_FAILED result. Unsafe layouts are errors.
pub(super) fn discover(config:&Config,programs:&[String])->Result<Toolchains>{
    let compiler_requested=programs.iter().any(|program|RUST_PROGRAMS.contains(&program.as_str())
        ||matches!(program.as_str(),"cmake"|"ninja"|"cl"|"link"|"lib"));
    let msvc=if compiler_requested&&config.execution.auto_discover_toolchains{
        crate::tools::windows_msvc::discover()
    }else{None};
    discover_with(config,programs,&Homes::current(),msvc)
}
pub(super) fn remap_path(path:&Path,mappings:&[(PathBuf,PathBuf)])->PathBuf{
    mappings.iter().filter_map(|(source,target)|path.strip_prefix(source).ok()
        .map(|relative|(source.components().count(),target.join(relative))))
        .max_by_key(|(depth,_)|*depth).map(|(_,path)|path).unwrap_or_else(||path.to_owned())
}
impl Toolchains{
    /// If one automatic SDK/MSVC leaf cannot have its native ACL edited, copy
    /// its selected family leaves together. The caller must seal the private
    /// tree before dropping its pins or starting any sandbox process.
    pub(super) fn prepare_readonly_copies(&mut self,private_tools:&Path,
        deadline:std::time::Instant)->Result<super::windows_tool_copy::StagedTools>{
        let staged=super::windows_tool_copy::stage_uneditable_roots(
            &self.automatic_roots,&self.copy_layouts,&self.copy_exclusions,private_tools,deadline)?;
        // Keep a shared relative layout without reading the installation
        // ancestor. Only rebase an ancestor environment variable when every
        // automatically selected leaf in that installation is in the copy.
        let complete_layouts=self.copy_layouts.iter().filter(|(source,_)|{
            let roots=self.automatic_roots.iter().filter(|root|root.starts_with(source)).collect::<Vec<_>>();
            !roots.is_empty()&&roots.iter().all(|root|staged.mappings.iter()
                .any(|(copied,_)|root.starts_with(copied)))
        }).map(|(source,namespace)|(source.clone(),private_tools.join(namespace))).collect::<Vec<_>>();
        self.remap_roots(&staged.mappings)?;
        for name in ["VCToolsInstallDir","WindowsSdkDir"]{
            if let Some(value)=self.environment.get_mut(std::ffi::OsStr::new(name)){
                *value=remap_path(Path::new(&*value),&complete_layouts).into_os_string();
            }
        }
        // These objects already inherit the private package SID. Their
        // separate profile-only seal supplies read-only enforcement; they
        // must never enter an external ACL lease which expects a fresh SID.
        self.readonly_roots.retain(|path|!staged.mappings.iter()
            .any(|(_,target)|path.starts_with(target)));
        self.readonly_roots.sort();self.readonly_roots.dedup();
        Ok(staged)
    }
    /// Create a fresh offline Cargo home without host configuration or tokens.
    /// This compatibility wrapper does not prewarm any dependency cache.
    pub(super) fn prepare_cargo_home(&self,private_home:&Path)->Result<()>{
        local_absolute(private_home)?;
        if let Some(host)=&self.cargo_home{
            if host.exists(){
                let host=canonical_local(host)?;
                let target=if private_home.exists(){native_path(private_home)?}else{private_home.to_owned()};
                if host.starts_with(&target)||target.starts_with(&host){
                    bail!("Private Cargo home must be separate from the host Cargo home");
                }
            }
        }
        fs::create_dir_all(private_home).context("Create per-job Cargo home")?;
        no_reparse(private_home)?;
        let private=Root::open(private_home).context("Open private Cargo home")?;
        private.replace("config.toml",b"[net]\noffline = true\n\n[registries.crates-io]\nprotocol = \"sparse\"\n",true,false)
            .context("Create isolated Cargo configuration")?;
        Ok(())
    }
    /// Copy only Cargo.lock's public crates.io archives and sparse index
    /// records. Missing archives/indexes stay absent so real Cargo decides
    /// which packages the current target needs. Cargo verifies/unpacks inside
    /// the sandbox; no synthetic checksums or host config/credentials/Git are used.
    pub(super) fn prepare_cargo_home_for_project(&self,private_home:&Path,project:&Path)->Result<()>{
        self.prepare_cargo_home(private_home)?;
        let Some(host)=&self.cargo_home else{return Ok(())};
        let packages=crates_io_locked_packages(project)?;
        // A missing lockfile gets a clean offline home too: Cargo can create a
        // no-dependency lockfile, or report missing packages without fetching.
        if packages.is_empty(){return Ok(());}
        if let Err(error)=no_reparse(host){
            if missing_file(&error){return Ok(());}
            return Err(error);
        }
        let host=Root::open(host).context("Open existing host Cargo cache without granting sandbox access")?;
        let private=Root::open(private_home)?;
        let candidates=match sparse_caches(&host){
            Ok(names)=>names,
            Err(error)if missing_file(&error)=>return Ok(()),
            Err(error)=>return Err(error),
        };
        let mut names=Vec::new();let mut total=0usize;
        // Each source namespace is verified independently. Incomplete caches
        // are useful too; another installed Cargo release may have its missing
        // archives. Never copy a Git index or an unverified registry config.
        for namespace in candidates{
            let source=format!("registry/index/{namespace}/config.json");
            let Some(bytes)=optional_cache_bytes(&host,&source,MAX_SETTINGS_BYTES as usize)?else{continue};
            let config=normalized_index_configuration(&bytes)?;
            copy_private_cache_bytes(&private,&source,&config,&mut total)?;
            names.push(namespace);
        }
        let mut copied_indexes=BTreeSet::new();
        for package in &packages{
            let filename=format!("{}-{}.crate",package.name,package.version);
            let mut archive=None;
            for namespace in &names{
                let source=format!("registry/cache/{namespace}/{filename}");
                let Some(bytes)=optional_cache_bytes(&host,&source,MAX_CARGO_CACHE_FILE_BYTES)?else{continue};
                if format!("{:x}",Sha256::digest(&bytes))!=package.checksum{
                    bail!("Cached crates.io archive {} {} does not match Cargo.lock's checksum",package.name,package.version);
                }
                // Inspect every existing copy: a valid archive in one cache
                // must not hide corruption or an unsafe link in another.
                if archive.is_none(){archive=Some(bytes);}
            }
            if let Some(bytes)=archive{
                for namespace in &names{
                    let target=format!("registry/cache/{namespace}/{filename}");
                    copy_private_cache_bytes(&private,&target,&bytes,&mut total)?;
                }
            }
            let entry=index_entry(&package.name);
            if copied_indexes.insert(entry.clone()){
                let mut selected:Option<(usize,Vec<u8>)>=None;
                for namespace in &names{
                    let source=format!("registry/index/{namespace}/.cache/{entry}");
                    let Some(bytes)=optional_cache_bytes(&host,&source,MAX_CARGO_CACHE_FILE_BYTES)?else{continue};
                    let available=verify_sparse_index(&bytes,&packages,&package.name)?;
                    if selected.as_ref().is_none_or(|(count,_)|available>*count){selected=Some((available,bytes));}
                }
                if let Some((_,bytes))=selected{
                    for namespace in &names{
                        let target=format!("registry/index/{namespace}/.cache/{entry}");
                        copy_private_cache_bytes(&private,&target,&bytes,&mut total)?;
                    }
                }
            }
        }
        host.unchanged_root()?;private.unchanged_root()?;
        Ok(())
    }
    /// A caller which copies immutable tools can remap an installation prefix
    /// while preserving its bin/lib/include layout. Values which are not paths
    /// remain unchanged; credentials and home variables are never introduced.
    pub(super) fn remap_roots(&mut self,mappings:&[(PathBuf,PathBuf)])->Result<()>{
        let mut mappings=mappings.to_vec();
        mappings.sort_by_key(|(source,_)|std::cmp::Reverse(source.components().count()));
        for (source,target) in &mappings{local_absolute(source)?;local_absolute(target)?;}
        let mapped=|path:&Path|->PathBuf{remap_path(path,&mappings)};
        for path in self.programs.values_mut(){*path=mapped(path);}
        for path in &mut self.path_entries{*path=mapped(path);}
        for path in &mut self.readonly_roots{*path=mapped(path);}
        for (key,value) in &mut self.environment{
            if matches!(key.to_str(),Some("LIB"|"LIBPATH"|"INCLUDE"|"PATH")){
                *value=env::join_paths(env::split_paths(&*value).map(|path|mapped(&path)))
                    .context("Remap private toolchain search paths")?;
            }else if Path::new(&*value).is_absolute(){*value=mapped(Path::new(&*value)).into_os_string();}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use std::ffi::OsStr;
    fn put(path:&Path,bytes:&[u8]){fs::create_dir_all(path.parent().unwrap()).unwrap();fs::write(path,bytes).unwrap();}
    fn base(path:&Path)->Config{
        let mut config=Config::default();
        config.execution.path=env::join_paths([path]).unwrap().into_string().unwrap();
        config
    }
    #[test]
    fn resolves_installed_rustup_tools_without_granting_homes_or_credentials(){
        let temp=tempfile::tempdir().unwrap();let home=temp.path().join("user");
        let cargo=home.join(".cargo");let rustup=home.join(".rustup");
        for name in ["cargo","rustc","rustup"]{put(&cargo.join("bin").join(format!("{name}.exe")),b"proxy");}
        put(&cargo.join("credentials.toml"),b"do not disclose");
        put(&cargo.join("config.toml"),b"host-only configuration");
        put(&rustup.join("settings.toml"),b"default_toolchain = 'stable-x86_64-pc-windows-msvc'\n");
        let installed=rustup.join("toolchains/stable-x86_64-pc-windows-msvc");
        for name in ["cargo","rustc","rustdoc"]{put(&installed.join("bin").join(format!("{name}.exe")),b"native tool");}
        fs::create_dir_all(installed.join("lib/rustlib")).unwrap();
        let registry=cargo.join("registry/src/index.crates.io-1949cf8c6b5b557f");
        fs::create_dir_all(&registry).unwrap();
        let homes=Homes{user:Some(home.clone()),cargo:Some(cargo.clone()),rustup:Some(rustup.clone()),toolchain:None};
        let result=discover_with(&base(&cargo.join("bin")),&["cargo".into(),"rustc".into()],&homes,None).unwrap();
        assert_eq!(result.programs["cargo"],native_path(&installed.join("bin/cargo.exe")).unwrap());
        assert_eq!(result.environment[OsStr::new("RUSTC")],native_path(&installed.join("bin/rustc.exe")).unwrap().into_os_string());
        for forbidden in [&home,&cargo,&rustup]{assert!(!result.readonly_roots.contains(&native_path(forbidden).unwrap()));}
        let cargo_root=native_path(&cargo).unwrap();
        assert!(!result.readonly_roots.iter().any(|root|root.starts_with(&cargo_root)));
        assert!(!result.environment.contains_key(OsStr::new("USERPROFILE")));
        assert!(!result.environment.contains_key(OsStr::new("RUSTUP_HOME")));
        let private=temp.path().join("private-cargo");result.prepare_cargo_home(&private).unwrap();
        let text=fs::read_to_string(private.join("config.toml")).unwrap();
        assert!(!text.contains("do not disclose")&&!text.contains("host-only configuration"));
        let config:toml::Value=toml::from_str(&text).unwrap();
        assert_eq!(config["net"]["offline"].as_bool(),Some(true));
        assert_eq!(config["registries"]["crates-io"]["protocol"].as_str(),Some("sparse"));
        assert!(config.get("source").is_none());
        assert_eq!(fs::read_dir(&private).unwrap().count(),1);
        assert!(result.prepare_cargo_home(&private).is_err(),"Existing private configuration must not be overwritten");
    }
    #[test]
    fn absent_tools_are_reported_as_missing_and_private_rustup_paths_are_rejected(){
        let temp=tempfile::tempdir().unwrap();let bin=temp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let result=discover_with(&base(&bin),&["missing".into()],&Homes::default(),None).unwrap();
        assert!(result.programs.is_empty()&&result.readonly_roots.is_empty());
        let rustup=temp.path().join("rustup");fs::create_dir_all(&rustup).unwrap();
        assert!(installed_rust_toolchain(&Homes{rustup:Some(rustup),toolchain:Some("../escape".into()),..Homes::default()}).is_err());
    }
    #[test]
    fn non_rust_tool_does_not_expose_registry_or_sibling_files(){
        let temp=tempfile::tempdir().unwrap();let bin=temp.path().join("tools");
        put(&bin.join("helper.exe"),b"native tool");put(&bin.join("secret.txt"),b"private");
        let cargo=temp.path().join("cargo");fs::create_dir_all(cargo.join("registry/src/index.crates.io-1949cf8c6b5b557f")).unwrap();
        let result=discover_with(&base(&bin),&["helper".into()],&Homes{cargo:Some(cargo),..Homes::default()},None).unwrap();
        assert_eq!(result.readonly_roots,vec![native_path(&bin.join("helper.exe")).unwrap()]);
        assert!(result.cargo_home.is_none()&&result.environment.is_empty());
    }
    #[test]
    fn explicit_native_readonly_roots_do_not_map_or_expose_user_home(){
        let temp=tempfile::tempdir().unwrap();let root=temp.path();let bin=root.join("bin");
        fs::create_dir_all(&bin).unwrap();let home=root.join("user");fs::create_dir_all(&home).unwrap();
        let mut config=base(&bin);
        config.execution.readonly_mounts.push(crate::config::ReadOnlyMount{source:home.clone(),target:home.clone()});
        assert!(discover_with(&config,&[],&Homes{user:Some(home),..Homes::default()},None).is_err());
        config.execution.readonly_mounts[0].source=bin;
        assert!(discover_with(&config,&[],&Homes::default(),None).is_err());
    }
    #[test]
    fn cache_recognition_excludes_git_and_private_registries(){
        assert!(recognized_sparse_crates_io("index.crates.io-1949cf8c6b5b557f"));
        assert!(!recognized_sparse_crates_io("github.com-1ecc6299db9ec823"));
        assert!(!recognized_sparse_crates_io("github.com-0123456789abcdef"));
        assert!(!recognized_sparse_crates_io("private-registry-1949cf8c6b5b557f"));
        assert!(!recognized_sparse_crates_io("index.crates.io-../../credentials"));
    }
    fn cached_tools(home:&Path)->Toolchains{
        Toolchains{programs:BTreeMap::new(),readonly_roots:vec![],path_entries:vec![],
            environment:BTreeMap::new(),cargo_home:Some(home.to_owned()),
            automatic_roots:Vec::new(),copy_exclusions:Vec::new(),copy_layouts:Vec::new()}
    }
    fn locked_project(path:&Path,name:&str,version:&str,archive:&[u8]){
        let checksum=format!("{:x}",Sha256::digest(archive));
        let text=format!("version = 4\n\n[[package]]\nname = {name:?}\nversion = {version:?}\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = {checksum:?}\n");
        put(&path.join("Cargo.lock"),text.as_bytes());
    }
    fn cached_package(home:&Path,namespace:&str,name:&str,version:&str,archive:&[u8]){
        put(&home.join(format!("registry/cache/{namespace}/{name}-{version}.crate")),archive);
        put(&home.join(format!("registry/index/{namespace}/config.json")),
            br#"{"dl":"https://static.crates.io/crates","api":"https://crates.io","host-private-field":"must not be copied"}"#);
        let entry=serde_json::json!({"name":name,"vers":version,"cksum":format!("{:x}",Sha256::digest(archive)),
            "deps":[],"features":{},"yanked":false});
        let mut bytes=vec![3,2,0,0,0];bytes.extend_from_slice(b"etag: fixture\0");
        bytes.extend_from_slice(version.as_bytes());bytes.push(0);
        bytes.extend_from_slice(entry.to_string().as_bytes());bytes.push(0);
        put(&home.join(format!("registry/index/{namespace}/.cache/{}",index_entry(name))),&bytes);
    }
    #[test]
    fn offline_cache_copies_only_locked_public_packages_without_credentials(){
        let temp=tempfile::tempdir().unwrap();let home=temp.path().join("cargo");
        let project=temp.path().join("project");let private=temp.path().join("private");
        let namespace="index.crates.io-1949cf8c6b5b557f";
        let archive=b"exact cached public archive";
        cached_package(&home,namespace,"example","1.2.3",archive);
        cached_package(&home,namespace,"unrelated","9.9.9",b"not requested");
        put(&home.join("credentials.toml"),b"private token");
        put(&home.join("config.toml"),b"private source replacement");
        put(&home.join("git/private-repository/secret.txt"),b"private git source");
        locked_project(&project,"example","1.2.3",archive);
        cached_tools(&home).prepare_cargo_home_for_project(&private,&project).unwrap();
        let copied=private.join(format!("registry/cache/{namespace}/example-1.2.3.crate"));
        assert_eq!(fs::read(&copied).unwrap(),archive);
        assert!(!private.join(format!("registry/cache/{namespace}/unrelated-9.9.9.crate")).exists());
        assert!(!private.join(format!("registry/index/{namespace}/.cache/{}",index_entry("unrelated"))).exists());
        for path in ["credentials.toml","git","registry/src"]{assert!(!private.join(path).exists());}
        let config=fs::read_to_string(private.join(format!("registry/index/{namespace}/config.json"))).unwrap();
        assert!(!config.contains("host-private-field")&&!config.contains("must not be copied"));
        assert_eq!(fs::read_to_string(private.join("config.toml")).unwrap(),
            "[net]\noffline = true\n\n[registries.crates-io]\nprotocol = \"sparse\"\n");
        // A real copy must not alias the host archive's data or permissions.
        fs::write(&copied,b"private change").unwrap();
        assert_eq!(fs::read(home.join(format!("registry/cache/{namespace}/example-1.2.3.crate"))).unwrap(),archive);
    }
    #[test]
    fn offline_no_dependency_lockfile_needs_no_host_cache(){
        let temp=tempfile::tempdir().unwrap();let project=temp.path().join("project");
        put(&project.join("Cargo.lock"),b"version = 4\n\n[[package]]\nname = 'local'\nversion = '0.0.0'\n");
        let private=temp.path().join("private");
        cached_tools(&temp.path().join("missing-host-cache"))
            .prepare_cargo_home_for_project(&private,&project).unwrap();
        assert_eq!(fs::read_dir(&private).unwrap().count(),1);
        assert!(!private.join("registry").exists());
    }
    #[test]
    fn offline_cache_rejects_corrupt_and_hardlinked_archives(){
        let temp=tempfile::tempdir().unwrap();let home=temp.path().join("cargo");
        let project=temp.path().join("project");let namespace="index.crates.io-1949cf8c6b5b557f";
        let archive=b"verified archive";
        cached_package(&home,namespace,"example","1.2.3",archive);
        locked_project(&project,"example","1.2.3",archive);
        let path=home.join(format!("registry/cache/{namespace}/example-1.2.3.crate"));
        let tools=cached_tools(&home);
        fs::write(&path,b"corrupted archive").unwrap();
        let error=tools.prepare_cargo_home_for_project(&temp.path().join("corrupt"),&project).unwrap_err();
        assert!(format!("{error:#}").contains("does not match Cargo.lock"));
        fs::remove_file(&path).unwrap();
        let outside=temp.path().join("outside.crate");fs::write(&outside,archive).unwrap();
        fs::hard_link(&outside,&path).unwrap();
        assert!(tools.prepare_cargo_home_for_project(&temp.path().join("hardlink"),&project).is_err());
    }
    #[test]
    fn offline_cache_allows_uncached_other_target_archives_and_split_namespaces(){
        let temp=tempfile::tempdir().unwrap();let home=temp.path().join("cargo");
        let project=temp.path().join("project");let private=temp.path().join("private");
        let namespaces=["index.crates.io-1949cf8c6b5b557f","index.crates.io-6f17d22bba15001f"];
        let packages=[("shared_dep",b"shared archive".as_slice()),
            ("windows_only",b"Windows archive".as_slice()),("unix_only",b"uncached Unix archive".as_slice())];
        cached_package(&home,namespaces[0],packages[0].0,"1.0.0",packages[0].1);
        cached_package(&home,namespaces[1],packages[1].0,"1.0.0",packages[1].1);
        cached_package(&home,namespaces[1],packages[2].0,"1.0.0",packages[2].1);
        fs::remove_file(home.join(format!("registry/cache/{}/unix_only-1.0.0.crate",namespaces[1]))).unwrap();
        put(&project.join("Cargo.toml"),br#"[package]
name = "offline_fixture"
version = "0.0.0"
[dependencies]
shared_dep = "1.0.0"
[target.'cfg(windows)'.dependencies]
windows_only = "1.0.0"
[target.'cfg(unix)'.dependencies]
unix_only = "1.0.0"
"#);
        let mut lock=String::from("version = 4\n\n[[package]]\nname = 'offline_fixture'\nversion = '0.0.0'\ndependencies = ['shared_dep', 'windows_only', 'unix_only']\n");
        for (name,archive) in packages{
            let checksum=format!("{:x}",Sha256::digest(archive));
            lock.push_str(&format!("\n[[package]]\nname = {name:?}\nversion = '1.0.0'\nsource = 'registry+https://github.com/rust-lang/crates.io-index'\nchecksum = {checksum:?}\n"));
        }
        put(&project.join("Cargo.lock"),lock.as_bytes());
        cached_tools(&home).prepare_cargo_home_for_project(&private,&project).unwrap();
        assert_eq!(fs::read_to_string(project.join("Cargo.lock")).unwrap(),lock,
            "Offline preparation must not remove another target's lockfile dependencies");
        for namespace in namespaces{
            for (name,archive) in &packages[..2]{
                assert_eq!(fs::read(private.join(format!("registry/cache/{namespace}/{name}-1.0.0.crate"))).unwrap(),*archive);
            }
            assert!(!private.join(format!("registry/cache/{namespace}/unix_only-1.0.0.crate")).exists());
            assert!(private.join(format!("registry/index/{namespace}/.cache/{}",index_entry("unix_only"))).is_file());
        }
        // Missing archives are intentionally left absent for Cargo to judge,
        // but any existing corrupt copy still fails even beside a valid one.
        put(&home.join(format!("registry/cache/{}/shared_dep-1.0.0.crate",namespaces[1])),b"corrupt second copy");
        let error=cached_tools(&home).prepare_cargo_home_for_project(&temp.path().join("corrupt-alternate"),&project).unwrap_err();
        assert!(format!("{error:#}").contains("does not match Cargo.lock"));
    }
    #[test]
    fn offline_cache_rejects_private_sources_and_untrusted_index_configuration(){
        let temp=tempfile::tempdir().unwrap();let project=temp.path().join("project");
        put(&project.join("Cargo.lock"),b"version = 4\n\n[[package]]\nname = 'private'\nversion = '1.0.0'\nsource = 'git+https://example.invalid/private'\n");
        assert!(crates_io_locked_packages(&project).is_err());
        for bytes in [br#"{"dl":"https://user:secret@static.crates.io/crates"}"#.as_slice(),
            br#"{"dl":"https://private.invalid/crates"}"#.as_slice(),
            br#"{"dl":"https://static.crates.io/crates","auth-required":true}"#.as_slice()]{
            assert!(normalized_index_configuration(bytes).is_err());
        }
        assert_eq!(index_entry("a"),"1/a");assert_eq!(index_entry("ab"),"2/ab");
        assert_eq!(index_entry("ABC"),"3/a/abc");assert_eq!(index_entry("example"),"ex/am/example");
    }
    #[test]
    fn offline_cache_preserves_existing_public_namespaces_and_byte_bound(){
        let temp=tempfile::tempdir().unwrap();let home=temp.path().join("cargo");
        let project=temp.path().join("project");let private=temp.path().join("private");
        let namespaces=["index.crates.io-1949cf8c6b5b557f","index.crates.io-6f17d22bba15001f"];
        for namespace in namespaces{cached_package(&home,namespace,"example","1.2.3",b"archive");}
        locked_project(&project,"example","1.2.3",b"archive");
        cached_tools(&home).prepare_cargo_home_for_project(&private,&project).unwrap();
        for namespace in namespaces{
            assert_eq!(fs::read(private.join(format!("registry/cache/{namespace}/example-1.2.3.crate"))).unwrap(),b"archive");
        }
        let private=Root::open(&private).unwrap();let mut total=MAX_CARGO_CACHE_TOTAL_BYTES;
        assert!(copy_private_cache_bytes(&private,"over-limit",b"x",&mut total).is_err());
        assert!(!private.path.join("over-limit").exists());
    }
    #[test]
    fn private_toolchain_remapping_preserves_executable_and_library_layout(){
        let temp=tempfile::tempdir().unwrap();let source=temp.path().join("source");let target=temp.path().join("private");
        let mut tools=Toolchains{programs:BTreeMap::from([("cl".into(),source.join("bin/cl.exe"))]),
            readonly_roots:vec![source.join("bin"),source.join("lib")],path_entries:vec![source.join("bin")],
            environment:BTreeMap::from([("CC".into(),source.join("bin/cl.exe").into_os_string()),
                ("LIB".into(),env::join_paths([source.join("lib"),source.join("sdk/lib")]).unwrap()),
                ("CARGO_NET_OFFLINE".into(),"true".into())]),cargo_home:None,
            automatic_roots:Vec::new(),copy_exclusions:Vec::new(),copy_layouts:Vec::new()};
        tools.remap_roots(&[(source,target.clone())]).unwrap();
        assert_eq!(tools.programs["cl"],target.join("bin/cl.exe"));
        assert_eq!(tools.environment[OsStr::new("CC")],target.join("bin/cl.exe").into_os_string());
        assert_eq!(env::split_paths(&tools.environment[OsStr::new("LIB")]).collect::<Vec<_>>(),
            vec![target.join("lib"),target.join("sdk/lib")]);
        assert_eq!(tools.environment[OsStr::new("CARGO_NET_OFFLINE")],OsString::from("true"));
    }
}
