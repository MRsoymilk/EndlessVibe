use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::{fs::{File, OpenOptions}, io::Write, path::{Path,PathBuf}, time::{SystemTime, UNIX_EPOCH}};
#[cfg(unix)] use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)] use std::os::windows::fs::OpenOptionsExt;
use fs4::fs_std::FileExt;
use subtle::ConstantTimeEq;

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
pub fn now_millis() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }
pub fn digest(bytes: impl AsRef<[u8]>) -> String { format!("{:x}", Sha256::digest(bytes.as_ref())) }
pub fn random_secret() -> Result<String> { let mut bytes = [0u8; 32]; getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("OS randomness unavailable: {e}"))?; Ok(URL_SAFE_NO_PAD.encode(bytes)) }
pub fn constant_eq(a: &str, b: &str) -> bool { bool::from(Sha256::digest(a.as_bytes()).as_slice().ct_eq(Sha256::digest(b.as_bytes()).as_slice())) }
pub fn html(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;") }
pub fn bounded_text(s: &str, max: usize) -> String { s.chars().take(max).collect() }

pub fn private_dir(path: &Path) -> Result<()> {
    if path.exists() { let md = std::fs::symlink_metadata(path)?; if !md.is_dir() || md.file_type().is_symlink() { bail!("State path must be a real directory"); } }
    else { std::fs::create_dir_all(path)?; }
    let md = std::fs::metadata(path)?;
    // The state directory belongs to the service user; do not chmod someone else's directory.
    if !crate::platform::owned_by_service(&md) { bail!("State directory must belong to the service user"); }
    crate::platform::apply_owner_directory_permissions(path)?;
    Ok(())
}

pub fn private_create(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)] options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(windows)] options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    let mut f = options.open(path).with_context(|| format!("Cannot create {} (existing files are never overwritten)", path.display()))?;
    f.write_all(bytes)?; f.sync_all()?; Ok(())
}

pub fn private_read(path: &Path, max: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    crate::platform::validate_private_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)] options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    #[cfg(windows)] options.custom_flags(0x0020_0000); // refuse reparse-point traversal
    let f = options.open(path)?;
    let md = f.metadata()?;
    if !md.is_file() || !crate::platform::single_link(&md) { bail!("Credential/state file changed during open"); }
    let mut bytes = Vec::new(); f.take((max + 1) as u64).read_to_end(&mut bytes)?; if bytes.len() > max { bail!("State file too large"); } Ok(bytes)
}

pub fn single_instance(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)] options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    #[cfg(windows)] options.custom_flags(0x0020_0000);
    let file = options.open(path)?;
    if !file.try_lock_exclusive().context("Lock service state directory")? {
        bail!("Another EndlessVibe process is using this state directory");
    }
    Ok(file)
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct ProcessIdentity{pub pid:i32,pub start_ticks:u64}
#[cfg(target_os="linux")]
fn process_start_ticks(pid:i32)->Result<u64>{
    if pid<=1{bail!("Invalid service PID");}let text=std::fs::read_to_string(format!("/proc/{pid}/stat"))?;let close=text.rfind(") ").context("Invalid /proc stat")?;let fields=text[close+2..].split_whitespace().collect::<Vec<_>>();let value=fields.get(19).context("/proc stat is missing start time")?;Ok(value.parse()?)
}
#[cfg(not(target_os="linux"))]
fn process_start_ticks(_pid:i32)->Result<u64>{bail!("PID start-time checks require Linux; use the service.lock on this platform")}
pub fn current_process_identity()->Result<ProcessIdentity>{let pid=std::process::id() as i32;Ok(ProcessIdentity{pid,start_ticks:process_start_ticks(pid)?})}
pub fn process_identity_alive(identity:ProcessIdentity)->bool{process_start_ticks(identity.pid).is_ok_and(|ticks|ticks==identity.start_ticks)}
fn service_pid_path(state_dir:&Path)->PathBuf{state_dir.join("service.pid")}
pub fn read_service_identity(state_dir:&Path)->Result<Option<ProcessIdentity>>{
    let path=service_pid_path(state_dir);let md=match std::fs::symlink_metadata(&path){Ok(md)=>md,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(error)=>return Err(error.into())};
    if !md.is_file()||md.file_type().is_symlink()||!crate::platform::single_link(&md)||!crate::platform::owned_by_service(&md)||!crate::platform::private_permissions(&md){bail!("service.pid must be an owner-only regular file");}
    let text=String::from_utf8(private_read(&path,128)?)?;let mut fields=text.split_whitespace();let pid:i32=fields.next().context("service.pid is missing PID")?.parse()?;let start_ticks:u64=fields.next().context("service.pid is missing process start time")?.parse()?;if fields.next().is_some(){bail!("service.pid has unexpected fields");}Ok(Some(ProcessIdentity{pid,start_ticks}))
}
pub fn clear_service_identity(state_dir:&Path,expected:ProcessIdentity)->Result<()>{
    if read_service_identity(state_dir)?.is_some_and(|value|value==expected){let path=service_pid_path(state_dir);match std::fs::remove_file(&path){Ok(())=>{},Err(error)if error.kind()==std::io::ErrorKind::NotFound=>{},Err(error)=>return Err(error.into())};}Ok(())
}
pub struct ServicePidGuard{state_dir:PathBuf,identity:ProcessIdentity}
impl Drop for ServicePidGuard{fn drop(&mut self){let _=clear_service_identity(&self.state_dir,self.identity);}}
#[cfg(target_os="linux")]
pub fn install_service_identity(state_dir:&Path)->Result<ServicePidGuard>{
    private_dir(state_dir)?;let identity=current_process_identity()?;if let Some(existing)=read_service_identity(state_dir)?{if process_identity_alive(existing){bail!("Another EndlessVibe process is already registered as PID {}",existing.pid);}clear_service_identity(state_dir,existing)?;}
    private_create(&service_pid_path(state_dir),format!("{} {}\n",identity.pid,identity.start_ticks).as_bytes())?;Ok(ServicePidGuard{state_dir:state_dir.to_owned(),identity})
}

#[cfg(not(target_os="linux"))]
pub fn install_service_identity(state_dir:&Path)->Result<ServicePidGuard>{
    // The held service.lock is the process identity on macOS/Windows. Do not
    // write unverified PID records when no /proc start-time check is available.
    private_dir(state_dir)?;
    Ok(ServicePidGuard{state_dir:state_dir.to_owned(),identity:ProcessIdentity{pid:0,start_ticks:0}})
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn digest_is_stable() { assert_eq!(digest("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"); }
    #[test] fn key_has_256_bits() { assert_eq!(random_secret().unwrap().len(), 43); assert_ne!(random_secret().unwrap(), random_secret().unwrap()); }
    #[test] fn escaping_is_safe() { assert_eq!(html("<\"&'>"), "&lt;&quot;&amp;&#39;&gt;"); }
    #[test] fn comparisons_work() { assert!(constant_eq("abc", "abc")); assert!(!constant_eq("abc", "abcd")); }
    #[cfg(target_os="linux")]
    #[test] fn process_identity_matches_current_process(){let identity=current_process_identity().unwrap();assert_eq!(identity.pid,std::process::id() as i32);assert!(process_identity_alive(identity));}
}
