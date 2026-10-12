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
/// Render Windows paths for humans without changing the OS-native path used
/// for file access, ACL checks, or canonicalization.
///
/// Windows canonicalize() can return \\?\C:\... or \\?\UNC\server\share.
/// Users should see C:\... and \\server\share instead. This function must
/// never be used for authorization decisions or in place of filesystem paths.
pub fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        windows_display_path(&text)
    } else {
        text.into_owned()
    }
}

fn windows_display_path(input: &str) -> String {
    let path = input.replace('/', "\\");
    let Some(rest) = path.strip_prefix(r"\\?\") else {
        return path;
    };
    let bytes = rest.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':' && bytes[2] == b'\\' {
        return rest.to_string();
    }
    if rest.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"UNC\")) {
        let unc = &rest[4..];
        let mut components = unc.split('\\');
        if matches!((components.next(), components.next()),
            (Some(server), Some(share)) if !server.is_empty() && !share.is_empty()) {
            return format!(r"\\{unc}");
        }
    }
    // Other namespaces (for example volume GUIDs and device paths) do not
    // have an equivalent ordinary path. Do not disguise their identity.
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_user_paths_are_unprefixed_for_display_only() {
        assert_eq!(windows_display_path(r"\\?\C:\Users\lww\AppData\Local\EndlessVibe\owner.key"),
            r"C:\Users\lww\AppData\Local\EndlessVibe\owner.key");
        assert_eq!(windows_display_path(r"C:\Users\lww\AppData\Roaming\EndlessVibe/config.toml"),
            r"C:\Users\lww\AppData\Roaming\EndlessVibe\config.toml");
        assert_eq!(windows_display_path(r"\\?\UNC\server\share\owner.key"),
            r"\\server\share\owner.key");
        assert_eq!(windows_display_path(r"\\?\unc\server\share/config.toml"),
            r"\\server\share\config.toml");
        assert_eq!(windows_display_path(r"\\?\Volume{GUID}\folder\file"),
            r"\\?\Volume{GUID}\folder\file");
        assert_eq!(windows_display_path(r"\\.\PhysicalDrive0"),
            r"\\.\PhysicalDrive0");
        assert_eq!(windows_display_path(r"\\?\UNC\server"),
            r"\\?\UNC\server");
    }

    #[test]
    fn display_paths_do_not_mutate_native_paths() {
        let path = Path::new(r"\\?\C:\Users\lww\AppData\Local\EndlessVibe\owner.key");
        let unchanged = path.to_path_buf();
        let rendered = display_path(path);
        assert_eq!(path, unchanged);
        if cfg!(windows) {
            assert_eq!(rendered, r"C:\Users\lww\AppData\Local\EndlessVibe\owner.key");
        } else {
            assert_eq!(rendered, unchanged.to_string_lossy());
        }
    }

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
