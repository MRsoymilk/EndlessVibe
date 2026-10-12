//! Linux directory-FD relative access. All symlinks, magic links and '..' are refused.
//! openat2 requires Linux >= 5.6; fail closed rather than fall back to string-prefix checks.
use anyhow::{bail, Context, Result};
use std::{ffi::{CString, OsStr}, fs::{File, OpenOptions}, io::{Read, Write}, os::{fd::{AsRawFd, FromRawFd}, unix::{ffi::OsStrExt, fs::{MetadataExt, OpenOptionsExt}}}, path::{Component, Path, PathBuf}};

#[repr(C)] struct OpenHow { flags: u64, mode: u64, resolve: u64 }
pub struct Root { directory: File, pub path: PathBuf }
pub struct Entry { pub name: String, pub kind: &'static str, pub bytes: Option<u64> }
fn cstring(s: &OsStr) -> Result<CString> { Ok(CString::new(s.as_bytes())?) }
fn open_beneath(directory: &File, path: &Path, flags: i32) -> Result<File> {
    let path = cstring(path.as_os_str())?;
    let how = OpenHow { flags: (flags | libc::O_CLOEXEC) as u64, mode: 0, resolve: 0x08 | 0x04 | 0x02 };
    // SAFETY: all pointers reference valid live data, and a successful owned fd is wrapped exactly once.
    let fd = unsafe { libc::syscall(libc::SYS_openat2, directory.as_raw_fd(), path.as_ptr(), &how as *const OpenHow, std::mem::size_of::<OpenHow>()) };
    if fd < 0 { return Err(std::io::Error::last_os_error()).context("Secure path open failed (requires Linux 5.6+, no symlinks or outside paths)"); }
    Ok(unsafe { File::from_raw_fd(fd as i32) })
}
pub fn denied_component(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    matches!(s.as_str(), ".git" | ".ssh" | ".gnupg" | ".aws" | ".azure" | ".kube" | ".netrc" | ".npmrc" | ".pypirc" | ".git-credentials" | "credentials.json" | "id_rsa" | "id_ed25519") || s == ".env" || s.starts_with(".env.") || s.starts_with(".endlessvibe-") || [".pem", ".key", ".p12", ".pfx"].iter().any(|suffix| s.ends_with(*suffix))
}
pub fn relative(s: &str, allow_root: bool) -> Result<PathBuf> {
    if (!allow_root && s.ends_with('/')) || s.len() > 4096 || s.contains(['\0', '\\', '\n', '\r']) { bail!("Invalid relative path"); }
    let p = Path::new(s);
    if s.is_empty() || p == Path::new(".") { if allow_root { return Ok(PathBuf::from(".")); } bail!("A file path is required"); }
    for c in p.components() { match c { Component::Normal(v) => { let name = v.to_str().context("Non-UTF8 path")?; if denied_component(name) { bail!("Sensitive or internal path is not accessible through file tools"); } }, Component::CurDir => {}, _ => bail!("Only project-relative paths without '..' are accepted") } }
    if p.file_name().is_none() { bail!("Invalid file path"); }
    Ok(p.to_owned())
}
impl Root {
    pub fn same_directory(&self, other: &Self) -> Result<bool> {
        Ok(crate::platform::same_file(&self.directory, &other.directory))
    }
    pub fn has_single_link(&self, path: &str) -> Result<bool> {
        let path = relative(path, false)?;
        let file = open_beneath(&self.directory, &path, libc::O_PATH)?;
        Ok(file.metadata()?.is_file() && crate::platform::single_link(&file))
    }
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize()?;
        if path.parent().is_none() { bail!("Filesystem root cannot be a workspace"); }
        let directory = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW).open(&path)?;
        // Probe openat2 now so a kernel/sandbox incompatibility is detected before serving requests.
        open_beneath(&directory, Path::new("."), libc::O_RDONLY | libc::O_DIRECTORY)?;
        Ok(Self { directory, path })
    }
    pub fn unchanged_root(&self) -> Result<()> { let a = self.directory.metadata()?; let b = std::fs::symlink_metadata(&self.path)?; if !b.is_dir() || a.dev() != b.dev() || a.ino() != b.ino() { bail!("Workspace root moved/replaced; restart to re-authorize it"); } Ok(()) }
    pub fn read(&self, path: &str, max: usize) -> Result<Vec<u8>> {
        let path = relative(path, false)?;
        let file = open_beneath(&self.directory, &path, libc::O_RDONLY | libc::O_NONBLOCK)?;
        let md = file.metadata()?;
        if !md.is_file() || md.nlink() != 1 { bail!("Only regular, single-link files are accessible"); }
        if md.len() > max as u64 { bail!("File exceeds maximum size ({max} bytes)"); }
        let mut out = Vec::new(); file.take(max as u64 + 1).read_to_end(&mut out)?;
        if out.len() > max { bail!("File grew beyond size limit"); } Ok(out)
    }
    pub fn read_optional(&self, path: &str, max: usize) -> Result<Option<Vec<u8>>> {
        match self.read(path, max) { Ok(v) => Ok(Some(v)), Err(e) if e.chain().any(|c| c.downcast_ref::<std::io::Error>().is_some_and(|i| i.kind() == std::io::ErrorKind::NotFound)) => Ok(None), Err(e) => Err(e) }
    }
    pub fn metadata(&self, path: &str) -> Result<std::fs::Metadata> { let p = relative(path, true)?; Ok(open_beneath(&self.directory, &p, libc::O_PATH)?.metadata()?) }
    pub fn directory_path(&self, path: &str) -> Result<PathBuf> { let p = relative(path, true)?; open_beneath(&self.directory, &p, libc::O_RDONLY | libc::O_DIRECTORY)?; self.unchanged_root()?; Ok(self.path.join(p)) }
    pub fn entries(&self, path: &str) -> Result<Vec<Entry>> {
        let p = relative(path, true)?;
        let directory = open_beneath(&self.directory, &p, libc::O_RDONLY | libc::O_DIRECTORY)?;
        // This /proc path is derived only from our owned fd, never from user input.
        let mut out = Vec::new();
        for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
            let entry = entry?; let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            if denied_component(&name) { continue; }
            let t = entry.file_type()?;
            let (kind, bytes) = if t.is_symlink() { ("symlink_blocked", None) } else if t.is_dir() { ("directory", None) } else if t.is_file() { ("file", entry.metadata().ok().map(|m|m.len())) } else { ("special_blocked", None) };
            out.push(Entry { name, kind, bytes }); if out.len() > 20000 { bail!("Directory is too large to list"); }
        }
        out.sort_by(|a,b|a.name.cmp(&b.name)); Ok(out)
    }
    pub fn create_directory(&self, path: &str) -> Result<()> {
        let p = relative(path, false)?;
        let mut dir = self.directory.try_clone()?;
        for part in p.components() { if let Component::Normal(name) = part { let c = cstring(name)?; let r = unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), 0o755) }; if r != 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists { return Err(std::io::Error::last_os_error().into()); } dir = open_beneath(&dir, Path::new(name), libc::O_RDONLY | libc::O_DIRECTORY)?; } }
        dir.sync_all()?; Ok(())
    }
    pub fn replace(&self, path: &str, bytes: &[u8], create_only: bool, create_parents: bool) -> Result<()> {
        let p = relative(path, false)?;
        let parent = p.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        if create_parents && parent != Path::new(".") { self.create_directory(parent.to_str().context("Invalid parent")?)?; }
        let dir = open_beneath(&self.directory, parent, libc::O_RDONLY | libc::O_DIRECTORY)?;
        let name = cstring(p.file_name().context("File name missing")?)?;
        let temp = CString::new(format!(".endlessvibe-{}.tmp", crate::util::random_secret()?))?;
        let mode = if create_only { 0o600 } else { self.metadata(path)?.mode() & 0o777 };
        let fd = unsafe { libc::openat(dir.as_raw_fd(), temp.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW, mode) };
        if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let result = (|| -> Result<()> {
            file.write_all(bytes)?; if unsafe { libc::fchmod(file.as_raw_fd(), mode) } != 0 { return Err(std::io::Error::last_os_error().into()); } file.sync_all()?;
            let flags: u32 = if create_only { libc::RENAME_NOREPLACE } else { 0 };
            let r = unsafe { libc::syscall(libc::SYS_renameat2, dir.as_raw_fd(), temp.as_ptr(), dir.as_raw_fd(), name.as_ptr(), flags) };
            if r < 0 { return Err(std::io::Error::last_os_error().into()); }
            dir.sync_all()?; Ok(())
        })();
        if result.is_err() { unsafe { libc::unlinkat(dir.as_raw_fd(), temp.as_ptr(), 0); } }
        result
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn relative_paths_are_strict() { for p in ["/etc/passwd", "../x", "a/../b", ".git/config", "src/.env", "x\\y", "a.key"] { assert!(relative(p, false).is_err(), "{p}"); } assert!(relative("src/main.rs", false).is_ok()); }
    #[test] fn safe_read_write_and_create() { let d = tempfile::tempdir().unwrap(); let root = Root::open(d.path()).unwrap(); root.replace("src/a.txt", b"hello", true, true).unwrap(); assert_eq!(root.read("src/a.txt", 20).unwrap(), b"hello"); assert!(root.replace("src/a.txt", b"bad", true, false).is_err()); root.replace("src/a.txt", b"new", false, false).unwrap(); assert_eq!(root.read("src/a.txt", 20).unwrap(), b"new"); }
    #[test] fn symlinks_and_hardlinks_are_blocked() { let d = tempfile::tempdir().unwrap(); let outside = tempfile::tempdir().unwrap(); std::fs::write(outside.path().join("x"), "secret").unwrap(); std::os::unix::fs::symlink(outside.path(), d.path().join("link")).unwrap(); std::fs::hard_link(outside.path().join("x"),d.path().join("hard")).unwrap(); let r = Root::open(d.path()).unwrap(); assert!(r.read("link/x", 100).is_err()); assert!(r.read("hard",100).is_err()); assert!(r.replace("link/x",b"bad",false,false).is_err()); }
    #[test] fn large_files_fail() { let d = tempfile::tempdir().unwrap(); std::fs::write(d.path().join("x"), "hello").unwrap(); assert!(Root::open(d.path()).unwrap().read("x", 2).is_err()); }
}
