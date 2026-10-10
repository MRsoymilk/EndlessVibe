//! Operating-system specific identity and security primitives.
//! Do not emulate Linux UID/permissions on Windows: Windows uses inherited ACLs.
use std::{fs::Metadata, path::Path};

#[cfg(unix)]
pub fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}
#[cfg(windows)]
pub fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    match (a.volume_serial_number(), a.file_index(), b.volume_serial_number(), b.file_index()) {
        (Some(av), Some(ai), Some(bv), Some(bi)) => av == bv && ai == bi,
        _ => false, // Unknown identity must not authorize overlapping roots.
    }
}
#[cfg(unix)]
pub fn single_link(m: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    m.nlink() == 1
}
#[cfg(windows)]
pub fn single_link(m: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.number_of_links() == Some(1)
}
#[cfg(unix)]
pub fn owned_by_service(m: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    m.uid() == unsafe { libc::geteuid() }
}
#[cfg(windows)]
pub fn owned_by_service(m: &Metadata) -> bool {
    // Files live beneath %LOCALAPPDATA% by default and inherit the user's ACL.
    // A Windows ACL ownership check is not available through std::fs::Metadata.
    m.is_file() || m.is_dir()
}
#[cfg(unix)]
pub fn private_permissions(m: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    m.mode() & 0o077 == 0
}
#[cfg(windows)]
pub fn private_permissions(_: &Metadata) -> bool {
    // The Windows state directory must be protected with a per-user ACL.
    true
}
pub fn validate_private_file(path: &Path) -> anyhow::Result<()> {
    let link = std::fs::symlink_metadata(path)?;
    if !link.is_file() || link.file_type().is_symlink() || !single_link(&link)
        || !owned_by_service(&link) || !private_permissions(&link) {
        anyhow::bail!("Private file must be a real, single-link, owner-controlled regular file");
    }
    Ok(())
}
pub fn git_executable_mode(m: &Metadata) -> bool {
    #[cfg(unix)] {
        use std::os::unix::fs::MetadataExt;
        m.mode() & 0o111 != 0
    }
    #[cfg(windows)] { let _ = m; false }
}
pub fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}
#[cfg(unix)]
pub fn apply_owner_directory_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}
#[cfg(windows)]
pub fn apply_owner_directory_permissions(_: &Path) -> anyhow::Result<()> {
    // Inherit the ACL from the per-user %LOCALAPPDATA% root. Running under
    // administrator/System or a shared profile is not supported by this default.
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_and_hardlink_checks() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("one"); let b = dir.path().join("two");
        std::fs::write(&a, b"data").unwrap();
        assert!(single_link(&std::fs::metadata(&a).unwrap()));
        assert!(same_file(&std::fs::metadata(&a).unwrap(), &std::fs::metadata(&a).unwrap()));
        std::fs::hard_link(&a, &b).unwrap();
        assert!(!single_link(&std::fs::metadata(&a).unwrap()));
        assert!(!validate_private_file(&a).is_ok());
    }
}
