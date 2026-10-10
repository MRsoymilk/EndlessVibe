// Copyright (c) 2026 YXWJ.
// All rights reserved.
//
// File: windows_recovery.rs
// Description: Recover orphaned, service-owned AppContainer identities after
// the Windows service has acquired its exclusive instance lock.

use anyhow::{bail, Context, Result};
use std::{fs, path::{Path,PathBuf}};
use windows_sys::Win32::Security::Isolation::DeleteAppContainerProfile;

const MAX_PENDING: usize = 128;
const PROFILE_PREFIX: &str = "EndlessVibe.Isolated.";
const MARKER_SUFFIX: &str = ".pending";
const HR_NOT_FOUND: u32 = 0x80070490;

/// The profile journal is outside all Projects and underneath the service's
/// protected private state directory.
pub fn journal_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("appcontainer-profiles")
}

fn valid_profile_name(value: &str) -> bool {
    let Some(digest) = value.strip_prefix(PROFILE_PREFIX) else {
        return false;
    };
    digest.len() == 32
        && digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Must run before the Win32 CreateAppContainerProfile API is called.
/// A crash between recording the marker and creating the profile is harmless:
/// the recovery routine also accepts an already-absent profile.
pub fn mark_pending(directory: &Path, profile_name: &str) -> Result<PathBuf> {
    if !valid_profile_name(profile_name) {
        bail!("Refusing to journal a profile outside the EndlessVibe namespace");
    }
    crate::util::private_dir(directory)?;
    let marker = directory.join(format!("{profile_name}{MARKER_SUFFIX}"));
    crate::util::private_create(&marker, format!("v1:{profile_name}\n").as_bytes())?;
    Ok(marker)
}

/// Recover ONLY profiles whose exact protected records are present. The
/// caller MUST hold service.lock exclusively and MUST NOT have started jobs.
///
/// All records are validated before the first profile is deleted to avoid
/// destroying valid entries on a partially tampered journal.
pub fn recover_pending(directory: &Path) -> Result<usize> {
    crate::util::private_dir(directory)?;
    let root = crate::security::paths::Root::open(directory)
        .context("Open protected AppContainer recovery journal")?;
    let entries = root.entries(".")?;
    if entries.len() > MAX_PENDING {
        bail!("Too many pending AppContainer records (maximum {MAX_PENDING})");
    }

    let mut verified = Vec::with_capacity(entries.len());
    for entry in entries {
        if entry.kind != "file" {
            bail!("AppContainer recovery records must be regular files");
        }
        let name = entry.name.strip_suffix(MARKER_SUFFIX)
            .context("Unexpected file in AppContainer recovery journal")?;
        if !valid_profile_name(name) {
            bail!("Recovery refused a profile outside the EndlessVibe namespace");
        }
        let record = root.read(&entry.name, 128)
            .context("Read capability-pinned AppContainer recovery record")?;
        if record != format!("v1:{name}\n").as_bytes() {
            bail!("AppContainer recovery record does not match its filename");
        }
        verified.push((name.to_owned(), entry.name));
    }

    let mut removed = 0usize;
    for (name, marker) in verified {
        let wide = name.encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();
        let hr = unsafe { DeleteAppContainerProfile(wide.as_ptr()) };
        if hr < 0 && hr as u32 != HR_NOT_FOUND {
            bail!("AppContainer profile cleanup failed: HRESULT 0x{:08x}", hr as u32);
        }
        // Remove the journal only after successful cleanup. Retain it on
        // errors so a future startup can retry or diagnose the failure.
        fs::remove_file(directory.join(&marker))
            .context("Remove recovered AppContainer profile journal entry")?;
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_are_exact_and_case_sensitive() {
        let valid = format!("{PROFILE_PREFIX}{}", "e".repeat(32));
        assert!(valid_profile_name(&valid));
        for bad in [
            format!("UnrelatedApp.{}", "a".repeat(32)),
            format!("{PROFILE_PREFIX}{}", "a".repeat(31)),
            format!("{PROFILE_PREFIX}{}", "A".repeat(32)),
            format!("{PROFILE_PREFIX}../../secrets"),
        ] {
            assert!(!valid_profile_name(&bad));
        }
    }

    #[test]
    fn interrupted_before_create_can_be_recovered_idempotently() {
        let temp = tempfile::tempdir().unwrap();
        let journal = journal_dir(temp.path());
        let name = format!("{PROFILE_PREFIX}{}", "e".repeat(32));
        let marker = mark_pending(&journal, &name).unwrap();
        assert!(marker.exists());
        assert_eq!(recover_pending(&journal).unwrap(), 1);
        assert_eq!(recover_pending(&journal).unwrap(), 0);
    }

    #[test]
    fn malformed_record_rejects_entire_recovery_batch() {
        let temp = tempfile::tempdir().unwrap();
        let dir = journal_dir(temp.path());
        let name = format!("{PROFILE_PREFIX}{}", "f".repeat(32));
        let valid = mark_pending(&dir, &name).unwrap();
        let invalid = dir.join("other.pending");
        fs::write(&invalid, b"not an EndlessVibe record").unwrap();
        assert!(recover_pending(&dir).is_err());
        assert!(valid.exists(),"Valid records are preserved if preflight rejects a batch");
        assert!(invalid.exists());
        fs::remove_file(&invalid).unwrap();
        assert_eq!(recover_pending(&dir).unwrap(),1);
    }
}
