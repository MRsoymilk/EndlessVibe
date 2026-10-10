//! Operating-system specific identity and security primitives.
//! Use file handles for identity and hard-link checks: Windows metadata methods
//! for volume/file IDs and link counts are still unstable on stable Rust.
use std::{fs::{File, Metadata}, path::Path};

/// Compare the identities of *opened* files/directories, never path strings.
#[cfg(unix)]
pub fn same_file(a: &File, b: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (a.metadata(), b.metadata()) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Stable Win32 handle information; do not use unstable windows_by_handle APIs.
#[cfg(windows)]
fn windows_file_info(
    file: &File,
) -> Option<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: The file owns a live OS handle and Win32 initializes the output
    // structure on success. Neither the handle nor the buffer outlives this call.
    let success = unsafe {
        GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr())
    };
    (success != 0).then(|| unsafe { info.assume_init() })
}

#[cfg(windows)]
pub fn same_file(a: &File, b: &File) -> bool {
    match (windows_file_info(a), windows_file_info(b)) {
        (Some(a), Some(b)) => {
            // Some filesystems cannot provide a stable file index (all zeroes).
            // Fail closed rather than treating distinct roots as identical.
            (a.nFileIndexHigh != 0 || a.nFileIndexLow != 0)
                && a.dwVolumeSerialNumber == b.dwVolumeSerialNumber
                && a.nFileIndexHigh == b.nFileIndexHigh
                && a.nFileIndexLow == b.nFileIndexLow
        }
        _ => false, // Failed handle inspection cannot authorize overlapping roots.
    }
}

/// Deny hard-linked files, including Windows files whose link counts are only
/// available via the Win32 handle API.
#[cfg(unix)]
pub fn single_link(file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata().is_ok_and(|md| md.is_file() && md.nlink() == 1)
}

#[cfg(windows)]
pub fn single_link(file: &File) -> bool {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    windows_file_info(file).is_some_and(|info| {
        info.nNumberOfLinks == 1 && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
    })
}

/// Reopen only when the caller has a path rather than a pinned file handle.
/// A no-follow open prevents reparse points/symlinks from being accepted.
/// Fail closed on missing files, access errors, or unknown link counts.
pub fn single_link_path(path: &Path) -> bool {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    options.open(path).is_ok_and(|file| single_link(&file))
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
    if !link.is_file() || link.file_type().is_symlink() || !single_link_path(path)
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
        let a = dir.path().join("one");
        let b = dir.path().join("two");
        let different = dir.path().join("different");
        std::fs::write(&a, b"data").unwrap();
        std::fs::write(&different, b"other").unwrap();
        let first = File::open(&a).unwrap();
        let again = File::open(&a).unwrap();
        assert!(single_link(&first));
        assert!(single_link_path(&a));
        assert!(same_file(&first, &again));
        assert!(!same_file(&first, &File::open(&different).unwrap()));
        drop(first);
        drop(again);
        std::fs::hard_link(&a, &b).unwrap();
        assert!(!single_link_path(&a));
        assert!(!single_link(&File::open(&b).unwrap()));
        assert!(same_file(&File::open(&a).unwrap(), &File::open(&b).unwrap()));
        assert!(validate_private_file(&a).is_err());
    }
}
