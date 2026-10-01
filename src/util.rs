use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::{fs::{File, OpenOptions}, io::Write, os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt}, path::Path, time::{SystemTime, UNIX_EPOCH}};
use subtle::ConstantTimeEq;

pub fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }
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
    if md.uid() != unsafe { libc::geteuid() } { bail!("State directory must be owned by the service user"); }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn private_create(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path).with_context(|| format!("Cannot create {} (existing files are never overwritten)", path.display()))?;
    f.write_all(bytes)?; f.sync_all()?; Ok(())
}

pub fn private_read(path: &Path, max: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC).open(path)?;
    let md = f.metadata()?;
    if !md.is_file() || md.nlink() != 1 || md.uid() != unsafe { libc::geteuid() } || md.mode() & 0o077 != 0 { bail!("Credential/state file must be a regular, single-link, owner-only file (chmod 600)"); }
    let mut bytes = Vec::new(); f.take((max + 1) as u64).read_to_end(&mut bytes)?; if bytes.len() > max { bail!("State file too large"); } Ok(bytes)
}

pub fn single_instance(path: &Path) -> Result<File> {
    use std::os::fd::AsRawFd;
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 { bail!("Another EndlessVibe process is using this state directory"); }
    Ok(file)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn digest_is_stable() { assert_eq!(digest("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"); }
    #[test] fn key_has_256_bits() { assert_eq!(random_secret().unwrap().len(), 43); assert_ne!(random_secret().unwrap(), random_secret().unwrap()); }
    #[test] fn escaping_is_safe() { assert_eq!(html("<\"&'>"), "&lt;&quot;&amp;&#39;&gt;"); }
    #[test] fn comparisons_work() { assert!(constant_eq("abc", "abc")); assert!(!constant_eq("abc", "abcd")); }
}
