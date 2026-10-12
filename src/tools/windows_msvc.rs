//! Discover a complete native x64 MSVC/Windows SDK installation.
//!
//! Both the acknowledged host backend and the LPAC policy layer consume the
//! same exact compiler paths. Discovery never starts an installer or shell.
use anyhow::{Context,Result};
use std::{collections::BTreeMap,env,ffi::OsString,fs,path::{Path,PathBuf}};
use tokio::process::Command;

#[derive(Clone,Debug)]
pub(super) struct Toolchain {
    pub(super) linker:PathBuf,
    pub(super) compiler:PathBuf,
    pub(super) librarian:PathBuf,
    vc_bin:PathBuf,
    vc_root:PathBuf,
    sdk_root:PathBuf,
    sdk_version:String,
}

fn version_key(path:&Path)->Vec<u32>{
    path.file_name().and_then(|s|s.to_str()).unwrap_or("")
        .split('.').map(|part|part.parse().unwrap_or(0)).collect()
}
fn versioned_children(root:&Path)->Vec<PathBuf>{
    let mut entries=fs::read_dir(root).into_iter().flatten().filter_map(|entry|entry.ok())
        .map(|entry|entry.path()).filter(|path|path.is_dir()).collect::<Vec<_>>();
    entries.sort_by_key(|path|std::cmp::Reverse(version_key(path)));
    entries
}
fn sdk_version(root:&Path)->Option<String>{
    versioned_children(&root.join("Lib")).into_iter().find(|dir|{
        let include=root.join("Include").join(dir.file_name().unwrap_or_default());
        dir.join("um/x64/kernel32.lib").is_file()&&
        dir.join("ucrt/x64/ucrt.lib").is_file()&&
        ["ucrt","shared","um"].iter().all(|part|include.join(part).is_dir())
    })?.file_name().map(|value|value.to_string_lossy().into_owned())
}
fn inspect(vc_root:PathBuf,sdk_root:PathBuf)->Option<Toolchain>{
    let vc_bin=vc_root.join("bin/Hostx64/x64");
    let linker=vc_bin.join("link.exe");
    let compiler=vc_bin.join("cl.exe");
    let librarian=vc_bin.join("lib.exe");
    if !(linker.is_file()&&compiler.is_file()&&librarian.is_file()
        &&vc_root.join("lib/x64/vcruntime.lib").is_file()
        &&vc_root.join("include").is_dir()) {return None;}
    let sdk_version=sdk_version(&sdk_root)?;
    Some(Toolchain{linker,compiler,librarian,vc_bin,vc_root,sdk_root,sdk_version})
}
fn program_files()->Vec<PathBuf>{
    let mut roots=Vec::new();
    for (key,fallback) in [("ProgramFiles",r"C:\Program Files"),
                           ("ProgramFiles(x86)",r"C:\Program Files (x86)")]{
        let root=env::var_os(key).map(PathBuf::from).unwrap_or_else(||fallback.into());
        if !roots.contains(&root){roots.push(root);}
    }
    roots
}
fn discover_from(visual_roots:&[PathBuf],sdk_roots:&[PathBuf],
    selected_linker:Option<&Path>,selected_vc:Option<&Path>)->Option<Toolchain>{
    if let Some(linker)=selected_linker{
        if !linker.is_absolute()||!linker.file_name().is_some_and(|name|name.eq_ignore_ascii_case("link.exe")){
            return None;
        }
        // .../VC/Tools/MSVC/<version>/bin/Hostx64/x64/link.exe
        let vc_root=linker.ancestors().nth(4)?;
        return sdk_roots.iter().find_map(|sdk|inspect(vc_root.to_owned(),sdk.clone())
            .filter(|found|found.linker==linker));
    }
    if let Some(vc)=selected_vc{
        if !vc.is_absolute(){return None;}
        return sdk_roots.iter().find_map(|sdk|inspect(vc.to_owned(),sdk.clone()));
    }
    for base in visual_roots{
        // VS 2026 can be installed beneath either its year or the 18 label.
        for release in ["18","2026","2022","17","2019","16"]{
            for edition in ["BuildTools","Community","Professional","Enterprise"]{
                let tools=base.join(release).join(edition).join("VC/Tools/MSVC");
                // An incomplete newer toolset must not hide a complete older
                // version in the same installation.
                for vc in versioned_children(&tools){
                    if let Some(found)=sdk_roots.iter().find_map(|sdk|inspect(vc.clone(),sdk.clone())){
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}
pub(super) fn discover()->Option<Toolchain>{
    let roots=program_files();
    let sdk_roots=if let Some(root)=env::var_os("WindowsSdkDir"){
        vec![PathBuf::from(root)]
    }else{roots.iter().map(|root|root.join("Windows Kits/10")).collect()};
    let visual_roots=roots.iter().map(|root|root.join("Microsoft Visual Studio")).collect::<Vec<_>>();
    let selected_linker=env::var_os("ENDLESSVIBE_MSVC_LINKER").map(PathBuf::from);
    let selected_vc=env::var_os("VCToolsInstallDir").map(PathBuf::from);
    discover_from(&visual_roots,&sdk_roots,selected_linker.as_deref(),selected_vc.as_deref())
}
impl Toolchain{
    fn sdk_subfolder(&self,part:&str)->PathBuf{
        self.sdk_root.join(part).join(&self.sdk_version)
    }
    fn library_paths(&self)->Vec<PathBuf>{
        vec![self.vc_root.join("lib/x64"),self.sdk_subfolder("Lib").join("um/x64"),
            self.sdk_subfolder("Lib").join("ucrt/x64")]
    }
    fn include_paths(&self)->Vec<PathBuf>{
        let mut paths=vec![self.vc_root.join("include")];
        for part in ["ucrt","shared","um","winrt","cppwinrt"]{
            let path=self.sdk_subfolder("Include").join(part);
            if path.is_dir(){paths.push(path);}
        }
        paths
    }
    pub(super) fn path_entries(&self)->Vec<PathBuf>{
        let mut paths=vec![self.vc_bin.clone()];
        let sdk_bin=self.sdk_subfolder("bin").join("x64");
        if sdk_bin.is_dir(){paths.push(sdk_bin);}
        paths
    }
    /// Exact versioned leaf directories, never ProgramFiles or an entire SDK.
    pub(super) fn readonly_roots(&self)->Vec<PathBuf>{
        let mut paths=self.path_entries();
        paths.extend(self.library_paths());paths.extend(self.include_paths());
        paths.sort();paths.dedup();paths
    }
    pub(super) fn environment(&self)->Result<BTreeMap<OsString,OsString>>{
        let lib=env::join_paths(self.library_paths()).context("Encode MSVC LIB paths")?;
        let include=env::join_paths(self.include_paths()).context("Encode MSVC INCLUDE paths")?;
        let mut values=BTreeMap::new();
        for (name,path) in [("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER",&self.linker),
            ("CC",&self.compiler),("CXX",&self.compiler),("AR",&self.librarian),
            ("CMAKE_C_COMPILER",&self.compiler),("CMAKE_CXX_COMPILER",&self.compiler),
            ("CMAKE_AR",&self.librarian),("VCToolsInstallDir",&self.vc_root),
            ("WindowsSdkDir",&self.sdk_root)]{
            values.insert(OsString::from(name),path.as_os_str().to_owned());
        }
        values.insert("LIB".into(),lib.clone());values.insert("LIBPATH".into(),lib);
        values.insert("INCLUDE".into(),include);
        values.insert("VCToolsVersion".into(),self.vc_root.file_name().unwrap_or_default().to_owned());
        values.insert("WindowsSDKVersion".into(),format!("{}\\",self.sdk_version).into());
        values.insert("WindowsSDKLibVersion".into(),format!("{}\\",self.sdk_version).into());
        values.insert("VSCMD_ARG_TGT_ARCH".into(),"x64".into());
        values.insert("VSCMD_ARG_HOST_ARCH".into(),"x64".into());
        let rc=self.sdk_subfolder("bin").join("x64/rc.exe");
        if rc.is_file(){values.insert("RC".into(),rc.into_os_string());}
        Ok(values)
    }
}

/// Preserve the host backend's existing environment contract. Its caller adds
/// explicit per-job environment after this helper; this is not an ACL grant.
pub(super) fn configure_host_job(command:&mut Command,base_path:&str)->bool{
    let Some(tc)=discover()else{return false};
    let Ok(environment)=tc.environment()else{return false};
    let mut bins=tc.path_entries();bins.extend(env::split_paths(base_path));
    let Ok(path)=env::join_paths(bins)else{return false};
    command.env("PATH",path).envs(environment);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn complete_fixture(vc:&Path,sdk:&Path){
        for file in ["bin/Hostx64/x64/link.exe","bin/Hostx64/x64/cl.exe",
                     "bin/Hostx64/x64/lib.exe","lib/x64/vcruntime.lib"]{
            let path=vc.join(file);fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path,b"fixture").unwrap();
        }
        fs::create_dir_all(vc.join("include")).unwrap();
        for file in ["Lib/10.0.26100.0/um/x64/kernel32.lib",
                     "Lib/10.0.26100.0/ucrt/x64/ucrt.lib","bin/10.0.26100.0/x64/rc.exe"]{
            let path=sdk.join(file);fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path,b"fixture").unwrap();
        }
        for part in ["ucrt","shared","um","winrt"]{
            fs::create_dir_all(sdk.join("Include/10.0.26100.0").join(part)).unwrap();
        }
    }
    #[test]
    fn discovers_versioned_sdk_and_complete_toolset(){
        let temp=tempfile::tempdir().unwrap();let root=temp.path();
        let vc=root.join("VC/Tools/MSVC/14.44.35207");let sdk=root.join("Windows Kits/10");
        complete_fixture(&vc,&sdk);
        let tc=inspect(vc.clone(),sdk.clone()).unwrap();
        assert_eq!(tc.sdk_version,"10.0.26100.0");
        assert!(tc.linker.ends_with("link.exe"));assert!(tc.compiler.ends_with("cl.exe"));
        assert!(tc.librarian.ends_with("lib.exe"));
        let roots=tc.readonly_roots();
        assert!(roots.contains(&vc.join("bin/Hostx64/x64")));
        assert!(roots.contains(&vc.join("lib/x64")));
        assert!(!roots.contains(&vc)&&!roots.contains(&sdk)&&!roots.contains(&root.to_owned()));
        let environment=tc.environment().unwrap();
        assert_eq!(environment.get(std::ffi::OsStr::new("CC")),Some(&tc.compiler.clone().into_os_string()));
        assert!(environment.contains_key(std::ffi::OsStr::new("INCLUDE")));
        assert!(!environment.contains_key(std::ffi::OsStr::new("PATH")));
    }
    #[test]
    fn discovers_modern_release_under_64_bit_program_files(){
        let temp=tempfile::tempdir().unwrap();
        let visual=temp.path().join("Program Files/Microsoft Visual Studio");
        let sdk=temp.path().join("Program Files (x86)/Windows Kits/10");
        let vc=visual.join("18/Community/VC/Tools/MSVC/14.50.10000");
        complete_fixture(&vc,&sdk);
        let incomplete=visual.join("18/Community/VC/Tools/MSVC/14.99.99999/bin/Hostx64/x64");
        fs::create_dir_all(&incomplete).unwrap();fs::write(incomplete.join("link.exe"),b"incomplete").unwrap();
        let tc=discover_from(&[visual],&[sdk],None,None).unwrap();
        assert_eq!(tc.vc_root,vc);
    }
    #[test]
    fn explicit_linker_selection_does_not_silently_fall_back(){
        let temp=tempfile::tempdir().unwrap();
        let visual=temp.path().join("Microsoft Visual Studio");
        let sdk=temp.path().join("sdk");
        let vc=visual.join("2022/BuildTools/VC/Tools/MSVC/14.44.1");
        complete_fixture(&vc,&sdk);
        assert!(discover_from(&[visual.clone()],&[sdk.clone()],Some(Path::new("relative/link.exe")),None).is_none());
        let linker=vc.join("bin/Hostx64/x64/link.exe");
        assert_eq!(discover_from(&[visual],&[sdk],Some(&linker),None).unwrap().linker,linker);
    }
    #[test]
    fn missing_toolset_or_windows_sdk_fails_closed(){
        let temp=tempfile::tempdir().unwrap();
        assert!(inspect(temp.path().join("vc"),temp.path().join("sdk")).is_none());
    }
}
