//! Durable evidence that a general AppContainer identity cannot still execute.
//!
//! A service-instance lock proves only that the old broker exited. It does not
//! prove that asynchronous Job Object termination has completed. An interrupted
//! `running` identity therefore requires a different verified Windows boot
//! sequence; restarting just the service intentionally does not unlock it.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet}, ffi::c_void, fs,
    mem::{size_of, zeroed}, path::{Path, PathBuf}, ptr,
    sync::{Arc, Mutex},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, LocalFree},
    Security::{FreeSid, Isolation::DeriveAppContainerSidFromAppContainerName,
        Authorization::ConvertSidToStringSidW},
    System::Threading::GetCurrentProcess,
};
use super::windows_acl::PrivateJournal;

const PREFIX: &str = "EndlessVibe.Isolated.";
const MAX_IDENTITIES: usize = 128;
const MAX_RUNS: u32 = 16;
const MAX_RECORD_BYTES: usize = 1024;
const MAX_ENTRIES: usize = MAX_IDENTITIES * (1 + 2 * MAX_RUNS as usize);
const SHARED_PAGE: usize = 0x7ffe_0000;
const SHARED_PREFIX_BYTES: usize = 0x2c8;
const BOOT_ID_OFFSET: usize = 0x2c4;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn ReadProcessMemory(process: HANDLE, base: *const c_void, buffer: *mut c_void,
        size: usize, read: *mut usize) -> i32;
}
#[repr(C)]
struct NativeVersion {
    size: u32, major: u32, minor: u32, build: u32, platform: u32,
    service_pack: [u16; 128],
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(version: *mut NativeVersion) -> i32;
}

fn pending(message: impl Into<String>) -> anyhow::Error {
    crate::error::coded("SANDBOX_CLEANUP_PENDING", false, message)
}
pub(crate) fn journal_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("appcontainer-execution")
}

/// This is the kernel's boot sequence, not a BCD boot-entry GUID and not an
/// estimate derived from wall time. KUSER_SHARED_DATA's documented BootId is
/// initialized by the OS loader. Read through an OS API so unsupported mappings
/// fail as errors instead of dereferencing an assumed address.
fn current_boot_id() -> Result<u32> {
    if !cfg!(target_arch = "x86_64") {
        bail!("Verified Windows boot recovery currently requires the x64 shared-data ABI");
    }
    let mut version: NativeVersion = unsafe { zeroed() };
    version.size = size_of::<NativeVersion>() as u32;
    let status = unsafe { RtlGetVersion(&mut version) };
    if status < 0 { bail!("Cannot verify Windows version for boot recovery (NTSTATUS 0x{:08x})", status as u32); }
    // Known Windows 10/11 and corresponding Server kernel families. Unknown
    // versions require explicit ABI validation instead of guessing offsets.
    if version.major != 10 || version.minor != 0 || !matches!(version.build,
        10240 | 10586 | 14393 | 15063 | 16299 | 17134 | 17763 | 18362 | 18363 |
        19041 | 19042 | 19043 | 19044 | 19045 | 20348 | 22000 | 22621 | 22631 |
        25398 | 26100 | 26200) {
        bail!("Windows {}.{}.{} has no verified boot-recovery shared-data ABI",
            version.major, version.minor, version.build);
    }
    fn read(version: &NativeVersion) -> Result<u32> {
        let mut bytes = [0u8; SHARED_PREFIX_BYTES];
        let mut received = 0usize;
        if unsafe { ReadProcessMemory(GetCurrentProcess(), SHARED_PAGE as *const c_void,
            bytes.as_mut_ptr() as _, bytes.len(), &mut received) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Read Windows kernel boot sequence");
        }
        if received != bytes.len() { bail!("Windows boot-recovery shared-data read was incomplete"); }
        let word = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let architecture = u16::from_le_bytes(bytes[0x26a..0x26c].try_into().unwrap());
        if architecture != 9 || word(0x26c) != version.major || word(0x270) != version.minor
            || word(0x260) & 0x0fff_ffff != version.build {
            bail!("Windows boot-recovery shared-data layout disagrees with native version/architecture");
        }
        Ok(word(BOOT_ID_OFFSET))
    }
    let first = read(&version)?;
    if read(&version)? != first { bail!("Windows kernel boot sequence changed while reading recovery evidence"); }
    Ok(first)
}

fn profile_sid(name: &str) -> Result<String> {
    let digest = name.strip_prefix(PREFIX).context("Execution record has an unrelated profile name")?;
    if digest.len() != 32 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        bail!("Execution record profile name is not canonical");
    }
    let wide = name.encode_utf16().chain(std::iter::once(0)).collect::<Vec<_>>();
    let mut sid = ptr::null_mut();
    let result = unsafe { DeriveAppContainerSidFromAppContainerName(wide.as_ptr(), &mut sid) };
    if result < 0 { bail!("Derive execution-record package SID failed (HRESULT 0x{:08x})", result as u32); }
    let mut text = ptr::null_mut();
    let converted = unsafe { ConvertSidToStringSidW(sid, &mut text) };
    unsafe { FreeSid(sid); }
    if converted == 0 { return Err(std::io::Error::last_os_error()).context("Render execution-record package SID"); }
    let rendered = (|| -> Result<String> {
        let mut length = 0;
        while length < 184 && unsafe { *text.add(length) } != 0 { length += 1; }
        if length == 184 { bail!("Execution-record package SID exceeded its bound"); }
        Ok(String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })?)
    })();
    unsafe { LocalFree(text as _); }
    rendered
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase { Prepared, Running, Empty }
impl Phase {
    fn label(self) -> &'static str { match self {
        Self::Prepared => "prepared", Self::Running => "running", Self::Empty => "empty",
    } }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    version: u32, name: String, sid: String, boot_id: u32, generation: u32, phase: Phase,
}
impl Entry {
    fn filename(&self) -> String {
        format!("{}.{:08x}.{}.json", self.name, self.generation, self.phase.label())
    }
    fn validate(&self, filename: &str) -> Result<()> {
        if self.version != 1 || self.generation > MAX_RUNS || filename != self.filename()
            || (self.phase == Phase::Prepared) != (self.generation == 0) {
            bail!("Execution recovery record has invalid version, phase, generation or filename");
        }
        if profile_sid(&self.name)? != self.sid {
            bail!("Execution recovery SID does not match its exact profile name");
        }
        Ok(())
    }
    fn same_identity(&self, other: &Self) -> bool {
        self.name == other.name && self.sid == other.sid && self.boot_id == other.boot_id
    }
}

struct State { current: Entry, entries: Vec<Entry>, uncertain: bool, finished: bool }

/// Append-only stage markers preserve older evidence until a newer marker has
/// been flushed. Generations support repeated launches by the same identity in
/// native tests; a newer running marker always overrides an older empty marker.
/// No Drop implementation removes evidence or guesses that a Job has exited.
pub(super) struct Record { journal: Arc<PrivateJournal>, state: Mutex<State> }
impl Record {
    pub(super) fn prepare(state_dir: &Path, name: &str, sid: &str) -> Result<Self> {
        if profile_sid(name)? != sid { bail!("Execution identity SID does not match its profile name"); }
        let entry = Entry { version: 1, name: name.to_owned(), sid: sid.to_owned(),
            boot_id: current_boot_id()?, generation: 0, phase: Phase::Prepared };
        let bytes = serde_json::to_vec(&entry)?;
        let journal = Arc::new(PrivateJournal::open(&journal_dir(state_dir))?);
        journal.create(&entry.filename(), &bytes)?;
        Ok(Self { journal, state: Mutex::new(State {
            current: entry.clone(), entries: vec![entry], uncertain: false, finished: false,
        }) })
    }
    fn append(&self, state: &mut State, entry: Entry) -> Result<()> {
        let result = (|| -> Result<()> {
            let bytes = serde_json::to_vec(&entry)?;
            if bytes.len() > MAX_RECORD_BYTES { bail!("Execution recovery record is oversized"); }
            self.journal.create(&entry.filename(), &bytes)?;
            Ok(())
        })();
        if let Err(error) = result {
            state.uncertain = true;
            return Err(pending(format!("Windows execution phase could not be durably recorded: {error:#}")));
        }
        state.current = entry.clone();
        state.entries.push(entry);
        Ok(())
    }
    /// Must complete before CreateProcess is called, including suspended starts.
    pub(super) fn mark_running(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| pending("Execution recovery coordinator poisoned"))?;
        if state.finished || state.uncertain || state.current.phase == Phase::Running {
            return Err(pending("Execution identity cannot enter another running phase"));
        }
        if state.current.generation >= MAX_RUNS { bail!("Execution identity exceeded its bounded launch count"); }
        let mut next = state.current.clone();
        next.generation += 1;
        next.phase = Phase::Running;
        self.append(&mut state, next)
    }
    /// Caller has independently verified root exit and ActiveProcesses == 0,
    /// or CreateProcess has failed without producing any process.
    pub(super) fn mark_empty(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| pending("Execution recovery coordinator poisoned"))?;
        if state.finished || state.uncertain { return Err(pending("Execution recovery phase is uncertain")); }
        if state.current.phase == Phase::Empty { return Ok(()); }
        if state.current.phase != Phase::Running { bail!("Execution identity has no running phase to close"); }
        let mut next = state.current.clone();
        next.phase = Phase::Empty;
        self.append(&mut state, next)
    }
    pub(super) fn cleanup_ready(&self) -> bool {
        self.state.lock().is_ok_and(|state|
            !state.uncertain && state.current.phase != Phase::Running)
    }
    /// Call only after external ACL revocation and profile cleanup both succeed.
    pub(super) fn finish(&self) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| pending("Execution recovery coordinator poisoned"))?;
        if state.finished { return Ok(()); }
        if state.uncertain || state.current.phase == Phase::Running {
            return Err(pending("Windows execution still requires verified process-tree cleanup"));
        }
        // Delete oldest evidence first. An interrupted cleanup leaves the most
        // recent safe phase intact, including when earlier stages are absent.
        while let Some(entry) = state.entries.first() {
            self.journal.remove(&entry.filename()).map_err(|error|
                pending(format!("Cannot remove completed execution recovery record: {error:#}")))?;
            state.entries.remove(0);
        }
        state.finished = true;
        Ok(())
    }
}

pub(crate) struct VerifiedRecovery {
    journal: Option<Arc<PrivateJournal>>, entries: Vec<Entry>, sids: BTreeSet<String>,
}
impl VerifiedRecovery {
    pub(crate) fn require_acl_sids(&self, sids: &BTreeSet<String>) -> Result<()> {
        if let Some(sid) = sids.iter().find(|sid| !self.sids.contains(*sid)) {
            return Err(pending(format!("Pending Windows ACL lease {sid} has no verified execution-exit record; recovery was not attempted")));
        }
        Ok(())
    }
    /// Consume this proof only after ACL and profile recovery have both finished.
    /// Dropping it on an earlier error intentionally preserves every marker.
    pub(crate) fn finish(self) -> Result<usize> {
        let Some(journal) = self.journal else { return Ok(0); };
        // Revalidate the whole batch before deleting any proof. The service
        // instance lock prevents a legitimate writer during startup recovery.
        for entry in &self.entries {
            let bytes = journal.read(&entry.filename(), MAX_RECORD_BYTES)?;
            let current: Entry = serde_json::from_slice(&bytes)?;
            if current != *entry { bail!("Execution recovery evidence changed before final cleanup"); }
        }
        for entry in self.entries { journal.remove(&entry.filename())?; }
        Ok(self.sids.len())
    }
}

fn load_records(state_dir: &Path) -> Result<(Option<Arc<PrivateJournal>>, Vec<Entry>)> {
    let directory = journal_dir(state_dir);
    match fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((None, Vec::new())),
        Err(error) => return Err(error).context("Inspect execution recovery journal"),
        Ok(_) => {},
    }
    let journal = Arc::new(PrivateJournal::open(&directory)?);
    let mut entries = Vec::new();
    let mut identities: BTreeMap<String, Entry> = BTreeMap::new();
    let mut sid_names = BTreeMap::new();
    for filename in journal.entries(MAX_ENTRIES)? {
        let bytes = journal.read(&filename, MAX_RECORD_BYTES)?;
        let entry: Entry = serde_json::from_slice(&bytes).context("Malformed Windows execution recovery record")?;
        entry.validate(&filename)?;
        if let Some(previous) = identities.get(&entry.name) {
            if !entry.same_identity(previous) { bail!("Execution stage records disagree on identity or boot sequence"); }
        } else {
            if identities.len() >= MAX_IDENTITIES { bail!("Too many pending execution identities"); }
            if sid_names.insert(entry.sid.clone(), entry.name.clone()).is_some() {
                bail!("Different execution identities claim the same package SID");
            }
            identities.insert(entry.name.clone(), entry.clone());
        }
        entries.push(entry);
    }
    entries.sort_by(|a, b| (&a.name, a.generation, a.phase).cmp(&(&b.name, b.generation, b.phase)));
    Ok((Some(journal), entries))
}

fn verify_records(journal: Option<Arc<PrivateJournal>>, entries: Vec<Entry>, boot_id: u32)
    -> Result<VerifiedRecovery> {
    let mut latest = BTreeMap::new();
    for entry in &entries { latest.insert(entry.name.clone(), entry); }
    let mut sids = BTreeSet::new();
    for entry in latest.values() {
        if entry.phase == Phase::Running && entry.boot_id == boot_id {
            return Err(pending(format!("Windows sandbox {} was running during this kernel boot. Restart Windows before recovery; restarting only EndlessVibe cannot prove that the old process tree exited. Its journals and ACL leases were preserved.", entry.name)));
        }
        sids.insert(entry.sid.clone());
    }
    Ok(VerifiedRecovery { journal, entries, sids })
}

/// Read and validate all execution evidence under the exclusive service lock,
/// before any ACL/profile recovery or new job. No record is removed here.
pub(crate) fn verify_recovery(state_dir: &Path) -> Result<VerifiedRecovery> {
    let (journal, entries) = load_records(state_dir)?;
    if entries.is_empty() { return verify_records(journal, entries, 0); }
    let boot_id = current_boot_id().map_err(|error|
        pending(format!("Cannot establish Windows boot identity for sandbox recovery: {error:#}")))?;
    verify_records(journal, entries, boot_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(byte: char) -> (String, String) {
        let name = format!("{PREFIX}{}", byte.to_string().repeat(32));
        let sid = profile_sid(&name).unwrap();
        (name, sid)
    }
    #[test]
    fn native_boot_sequence_query_is_stable() {
        let first = current_boot_id().unwrap();
        println!("WINDOWS_KERNEL_BOOT_SEQUENCE={first}");
        for _ in 0..4 { assert_eq!(current_boot_id().unwrap(), first); }
    }
    #[test]
    fn prepared_and_empty_are_safe_but_same_boot_running_is_blocked() {
        let temp = tempfile::tempdir().unwrap();
        let (name, sid) = identity('a');
        let record = Record::prepare(temp.path(), &name, &sid).unwrap();
        assert!(record.cleanup_ready());
        verify_recovery(temp.path()).unwrap().require_acl_sids(&BTreeSet::from([sid.clone()])).unwrap();
        record.mark_running().unwrap();
        assert!(!record.cleanup_ready());
        let error = match verify_recovery(temp.path()) { Ok(_) => panic!("running identity was accepted"), Err(error) => error };
        assert_eq!(crate::error::code(&error), "SANDBOX_CLEANUP_PENDING");
        assert!(record.finish().is_err());
        record.mark_empty().unwrap();
        assert!(record.cleanup_ready());
        verify_recovery(temp.path()).unwrap().require_acl_sids(&BTreeSet::from([sid])).unwrap();
        // A later launch must override the earlier durable empty marker.
        record.mark_running().unwrap();
        assert!(verify_recovery(temp.path()).is_err());
        record.mark_empty().unwrap();
        record.finish().unwrap();
        assert_eq!(fs::read_dir(journal_dir(temp.path())).unwrap().count(), 0);
    }
    #[test]
    fn different_boot_running_can_recover_without_a_wall_clock_guess() {
        let temp = tempfile::tempdir().unwrap();
        let (name, sid) = identity('b');
        let record = Record::prepare(temp.path(), &name, &sid).unwrap();
        record.mark_running().unwrap();
        let (journal, entries) = load_records(temp.path()).unwrap();
        let boot = entries[0].boot_id;
        let proof = verify_records(journal, entries, boot.wrapping_add(1)).unwrap();
        proof.require_acl_sids(&BTreeSet::from([sid])).unwrap();
        assert_eq!(proof.finish().unwrap(), 1);
    }
    #[test]
    fn malformed_or_missing_exit_evidence_rejects_the_batch() {
        let temp = tempfile::tempdir().unwrap();
        let proof = verify_recovery(temp.path()).unwrap();
        assert!(proof.require_acl_sids(&BTreeSet::from([identity('c').1])).is_err());
        let (name, sid) = identity('c');
        let record = Record::prepare(temp.path(), &name, &sid).unwrap();
        let path = journal_dir(temp.path()).join("unexpected.json");
        fs::write(&path, b"invalid record").unwrap();
        assert!(verify_recovery(temp.path()).is_err());
        assert_eq!(fs::read_dir(journal_dir(temp.path())).unwrap().count(), 2);
        fs::remove_file(path).unwrap();
        record.finish().unwrap();
    }
    #[test]
    fn partial_final_cleanup_keeps_the_newest_empty_proof() {
        let temp = tempfile::tempdir().unwrap();
        let (name, sid) = identity('d');
        let record = Record::prepare(temp.path(), &name, &sid).unwrap();
        record.mark_running().unwrap();
        record.mark_empty().unwrap();
        let (journal, entries) = load_records(temp.path()).unwrap();
        let journal = journal.unwrap();
        for entry in &entries[..entries.len() - 1] { journal.remove(&entry.filename()).unwrap(); }
        let proof = verify_recovery(temp.path()).unwrap();
        proof.require_acl_sids(&BTreeSet::from([sid])).unwrap();
        assert_eq!(proof.finish().unwrap(), 1);
    }
}
