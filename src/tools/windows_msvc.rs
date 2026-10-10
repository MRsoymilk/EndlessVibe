//! Discover a host MSVC x64 toolchain for Windows Cargo jobs.
//!
//! The service uses an intentionally minimal environment. MSVC builds need
//! their private compiler tools, Windows SDK libraries, and headers explicitly
//! configured even when link.exe is installed.
use std::{env,fs,path::{Path,PathBuf}};
use tokio::process::Command;

#[derive(Clone,Debug)]
struct Toolchain {
    linker:PathBuf,
    compiler:PathBuf,
    librarian:PathBuf,
    vc_bin:PathBuf,
    vc_root:PathBuf,
    sdk_root:PathBuf,
    sdk_version:String,
}

fn version_key(path:&Path)->Vec<u32>{
    path.file_name().and_then(|s|s.to_str()).unwrap_or("")
        .split('.').map(|part|part.parse().unwrap_or(0)).collect()
}
fn newest_child(root:&Path,mut allowed:impl FnMut(&Path)->bool)->Option<PathBuf>{
    let mut matches=fs::read_dir(root).ok()?.filter_map(|entry|entry.ok().map(|e|e.path()))
        .filter(|path|path.is_dir()&&allowed(path)).collect::<Vec<_>>();
    matches.sort_by_key(|path|version_key(path));
    matches.pop()
}
fn sdk_version(root:&Path)->Option<String>{
    let dir=newest_child(&root.join("Lib"),|dir|{
        dir.join("um/x64/kernel32.lib").is_file()&&
        dir.join("ucrt/x64/ucrt.lib").is_file()&&
        root.join("Include").join(dir.file_name().unwrap_or_default()).join("ucrt").is_dir()
    })?;
    dir.file_name().map(|value|value.to_string_lossy().into_owned())
}
fn inspect(vc_root:PathBuf,sdk_root:PathBuf)->Option<Toolchain>{
    let vc_bin=vc_root.join("bin/Hostx64/x64");
    let linker=vc_bin.join("link.exe");
    let compiler=vc_bin.join("cl.exe");
    let librarian=vc_bin.join("lib.exe");
    if !(linker.is_file()&&compiler.is_file()&&librarian.is_file()
        &&vc_root.join("lib/x64/vcruntime.lib").is_file()) {return None;}
    let sdk_version=sdk_version(&sdk_root)?;
    Some(Toolchain{linker,compiler,librarian,vc_bin,vc_root,sdk_root,sdk_version})
}
fn program_files_x86()->PathBuf{
    env::var_os("ProgramFiles(x86)").map(PathBuf::from)
        .unwrap_or_else(||PathBuf::from(r"C:\Program Files (x86)"))
}
fn discover()->Option<Toolchain>{
    let program_files=program_files_x86();
    let sdk_root=env::var_os("WindowsSdkDir").map(PathBuf::from)
        .filter(|path|path.join("Lib").is_dir())
        .unwrap_or_else(||program_files.join("Windows Kits/10"));
    if let Some(linker)=env::var_os("ENDLESSVIBE_MSVC_LINKER").map(PathBuf::from){
        // .../VC/Tools/MSVC/<version>/bin/Hostx64/x64/link.exe
        if linker.file_name().is_some_and(|name|name.eq_ignore_ascii_case("link.exe")){
            if let Some(vc_root)=linker.ancestors().nth(4){
                if let Some(found)=inspect(vc_root.to_owned(),sdk_root.clone()){
                    if found.linker==linker {return Some(found);}
                }
            }
        }
    }
    for edition in ["BuildTools","Community","Professional","Enterprise"]{
        for year in ["2022","2026","2019"]{
            let base=program_files.join("Microsoft Visual Studio").join(year)
                .join(edition).join("VC/Tools/MSVC");
            if let Some(toolset)=newest_child(&base,|d|{
                d.join("bin/Hostx64/x64/link.exe").is_file()
            }){
                if let Some(found)=inspect(toolset,sdk_root.clone()){return Some(found);}
            }
        }
    }
    None
}
fn sdk_subfolder(toolchain:&Toolchain,part:&str)->PathBuf{
    toolchain.sdk_root.join(part).join(&toolchain.sdk_version)
}
fn path_env(paths:impl IntoIterator<Item=PathBuf>)->Option<std::ffi::OsString>{
    env::join_paths(paths).ok()
}

/// Complete the clean host environment for the **existing** MSVC linker.
/// Job Objects are handled separately and do not supply file-system isolation.
/// Explicit per-job environment keys are applied after this helper.
pub(super) fn configure_host_job(command:&mut Command,base_path:&str)->bool{
    let Some(tc)=discover()else{return false};
    let lib_paths=[
        tc.vc_root.join("lib/x64"),
        sdk_subfolder(&tc,"Lib").join("um/x64"),
        sdk_subfolder(&tc,"Lib").join("ucrt/x64"),
    ];
    let mut includes=vec![
        tc.vc_root.join("include"),
        sdk_subfolder(&tc,"Include").join("ucrt"),
        sdk_subfolder(&tc,"Include").join("shared"),
        sdk_subfolder(&tc,"Include").join("um"),
    ];
    let winrt=sdk_subfolder(&tc,"Include").join("winrt");
    if winrt.is_dir(){includes.push(winrt);}
    let sdk_bin=sdk_subfolder(&tc,"bin").join("x64");
    let mut bins=vec![tc.vc_bin.clone()];
    if sdk_bin.is_dir(){bins.push(sdk_bin.clone());}
    bins.extend(env::split_paths(base_path));
    let Some(path)=path_env(bins)else{return false};
    let Some(lib)=path_env(lib_paths)else{return false};
    let Some(include)=path_env(includes)else{return false};
    command.env("PATH",path)
        .env("LIB",&lib)
        .env("INCLUDE",include)
        .env("LIBPATH",lib)
        .env("CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER",&tc.linker)
        .env("CC",&tc.compiler)
        .env("CXX",&tc.compiler)
        .env("AR",&tc.librarian)
        .env("CMAKE_C_COMPILER",&tc.compiler)
        .env("CMAKE_CXX_COMPILER",&tc.compiler)
        .env("CMAKE_AR",&tc.librarian)
        .env("VCToolsInstallDir",&tc.vc_root)
        .env("VCToolsVersion",tc.vc_root.file_name().unwrap_or_default())
        .env("WindowsSdkDir",&tc.sdk_root);
    let rc=sdk_bin.join("rc.exe");
    if rc.is_file(){command.env("RC",rc);}
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_versioned_sdk_and_complete_toolset(){
        let temp=tempfile::tempdir().unwrap();
        let root=temp.path();
        let vc=root.join("VC/Tools/MSVC/14.44.35207");
        let sdk=root.join("Windows Kits/10");
        for file in ["bin/Hostx64/x64/link.exe","bin/Hostx64/x64/cl.exe",
                     "bin/Hostx64/x64/lib.exe","lib/x64/vcruntime.lib"]{
            let path=vc.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path,b"fixture").unwrap();
        }
        for file in ["Lib/10.0.26100.0/um/x64/kernel32.lib",
                     "Lib/10.0.26100.0/ucrt/x64/ucrt.lib"]{
            let path=sdk.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path,b"fixture").unwrap();
        }
        fs::create_dir_all(sdk.join("Include/10.0.26100.0/ucrt")).unwrap();
        let tc=inspect(vc,sdk).unwrap();
        assert_eq!(tc.sdk_version,"10.0.26100.0");
        assert!(tc.linker.ends_with("link.exe"));
        assert!(tc.compiler.ends_with("cl.exe"));
        assert!(tc.librarian.ends_with("lib.exe"));
    }
    #[test]
    fn missing_toolset_or_windows_sdk_fails_closed(){
        let temp=tempfile::tempdir().unwrap();
        assert!(inspect(temp.path().join("vc"),temp.path().join("sdk")).is_none());
    }
}
