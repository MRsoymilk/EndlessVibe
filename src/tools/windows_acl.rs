//! Per-job AppContainer ACL leases for explicitly authorized local trees.
//!
//! The broker changes only ACEs for a fresh package SID. It never restores an
//! old DACL, grants ownership/security editing, follows reparse points, or
//! recursively invokes a path-based ACL API. The caller must keep its Project
//! lock until the contained process tree is empty and `finish` has returned.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet}, ffi::{c_void, OsString}, fs::{self, File, OpenOptions}, io::{Read, Write},
    mem::{size_of, zeroed}, os::windows::{ffi::{OsStrExt, OsStringExt},
        fs::OpenOptionsExt, io::{AsRawHandle, FromRawHandle, OwnedHandle}},
    path::{Component, Path, PathBuf, Prefix}, ptr, sync::{Arc, Mutex, OnceLock, Weak},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, LocalFree},
    Security::{
        AddAccessAllowedAceEx, AddAccessDeniedAceEx, AddAce, EqualSid, GetAce,
        GetLengthSid, GetSecurityDescriptorControl, GetSidSubAuthority,
        GetSidSubAuthorityCount, GetTokenInformation, InitializeAcl, IsValidAcl, IsValidSid,
        MapGenericMask, ACL, GENERIC_MAPPING, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, TOKEN_USER, TokenUser,
        InitializeSecurityDescriptor, SetSecurityDescriptorControl, SetSecurityDescriptorDacl,
        Authorization::{ConvertSidToStringSidW, ConvertStringSidToSidW,
            ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SE_FILE_OBJECT},
        Isolation::GetAppContainerFolderPath,
        TOKEN_QUERY,
    },
    Storage::FileSystem::{
        CreateDirectoryW, GetFileAttributesW, GetFileInformationByHandle, GetFinalPathNameByHandleW,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, INVALID_FILE_ATTRIBUTES,
    },
    System::{Com::CoTaskMemFree, Threading::{GetCurrentProcess, OpenProcessToken}},
};

const MAX_ROOTS: usize = 128;
const MAX_NODES: usize = 200_000;
const MAX_DEPTH: usize = 96;
const MAX_PENDING: usize = 128;
const MAX_JOURNAL_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
const READ_CONTROL: u32 = 0x0002_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const DELETE: u32 = 0x0001_0000;
const DELETE_CHILD: u32 = 0x40;
const READ_ATTRIBUTES: u32 = 0x80;
const READ_EXECUTE: u32 = 0x0012_00a9;
const READ_WRITE: u32 = 0x0013_01bf;
const WRITE_EFFECTS: u32 = 0x116 | DELETE | DELETE_CHILD | WRITE_DAC | WRITE_OWNER;
const SECURITY_EFFECTS: u32 = WRITE_DAC | WRITE_OWNER | DELETE_CHILD;
const INHERIT: u32 = 0x03;
const INHERITED: u8 = 0x10;
const DACL_INFORMATION: u32 = 0x04;
const DACL_PROTECTED: u16 = 0x1000;
const DACL_AUTO_INHERIT_REQ: u16 = 0x0100;
const DACL_AUTO_INHERITED: u16 = 0x0400;
const DACL_CONTROL_BITS: u16 = DACL_PROTECTED | 0x050c;
const ACL_REVISION_DS_VALUE: u32 = 4;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CompareStringOrdinal(a: *const u16, a_len: i32, b: *const u16,
        b_len: i32, ignore_case: i32) -> i32;
}
#[link(name = "userenv")]
unsafe extern "system" {
    fn GetUserProfileDirectoryW(token: HANDLE, directory: *mut u16, length: *mut u32) -> i32;
}
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSetSecurityObject(handle: HANDLE, information: u32, descriptor: *mut c_void) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access { ReadExecute, ReadWrite }

#[derive(Clone, Debug)]
pub struct Grant { pub path: PathBuf, pub access: Access }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity { volume: u32, index: u64 }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedGrant { path: String, access: Access, identity: Identity, directory: bool }

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record { version: u32, lease: String, sid: String, grants: Vec<RecordedGrant> }

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) { if !self.0.is_null() { unsafe { LocalFree(self.0); } } }
}

struct Sid { words: Vec<u32> }
impl Sid {
    fn parse(text: &str) -> Result<Self> {
        if text.len() > 184 || text.contains('\0') { bail!("Invalid AppContainer lease SID"); }
        let wide = wide(text);
        let mut raw = ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut raw) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Parse AppContainer lease SID");
        }
        let allocation = LocalAllocation(raw);
        if unsafe { IsValidSid(raw) } == 0
            || unsafe { *GetSidSubAuthorityCount(raw) } != 8
            || unsafe { *GetSidSubAuthority(raw, 0) } != 2
            || !text.starts_with("S-1-15-2-") {
            bail!("ACL leases require a unique AppContainer package SID, not a capability/group SID");
        }
        let mut canonical = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(raw, &mut canonical) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Normalize AppContainer SID");
        }
        let rendered = LocalAllocation(canonical as _);
        let mut length = 0;
        while length < 184 && unsafe { *canonical.add(length) } != 0 { length += 1; }
        if length == 184 || String::from_utf16(unsafe {
            std::slice::from_raw_parts(canonical, length)
        })? != text { bail!("AppContainer SID must use its canonical representation"); }
        drop(rendered);
        let bytes = unsafe { GetLengthSid(raw) } as usize;
        let mut words = vec![0u32; bytes.div_ceil(4)];
        unsafe { ptr::copy_nonoverlapping(raw as *const u8, words.as_mut_ptr() as *mut u8, bytes); }
        drop(allocation);
        Ok(Self { words })
    }
    fn raw(&self) -> *mut c_void { self.words.as_ptr() as *mut c_void }
}

fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() }
fn path_wide(path: &Path) -> Result<Vec<u16>> {
    let mut value = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if value.is_empty() || value.len() > 32760 || value.contains(&0) {
        bail!("ACL lease path is empty, oversized or contains NUL");
    }
    value.push(0);
    Ok(value)
}
fn key(path: &Path) -> Result<String> {
    let text = path.to_str().context("ACL lease paths must be Unicode")?;
    Ok(text.strip_prefix(r"\\?\").unwrap_or(text).replace('/', "\\")
        .trim_end_matches('\\').to_ascii_lowercase())
}
fn within(path: &Path, parent: &Path) -> Result<bool> {
    let path = key(path)?.encode_utf16().collect::<Vec<_>>();
    let parent = key(parent)?.encode_utf16().collect::<Vec<_>>();
    if path.len() < parent.len() { return Ok(false); }
    let equal = unsafe { CompareStringOrdinal(path.as_ptr(), parent.len() as i32,
        parent.as_ptr(), parent.len() as i32, 1) } == 2;
    Ok(equal && (path.len() == parent.len() || path[parent.len()] == b'\\' as u16))
}
fn equal_path(a: &Path, b: &Path) -> Result<bool> { Ok(within(a, b)? && within(b, a)?) }

fn identity(file: &File) -> Result<(Identity, bool, u32)> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Read pinned ACL object identity");
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        bail!("AppContainer ACL leases refuse reparse points and junctions");
    }
    let value = Identity { volume: info.dwVolumeSerialNumber,
        index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64 };
    if value.volume == 0 || value.index == 0 { bail!("ACL lease object has no stable local file identity"); }
    Ok((value, info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0, info.nNumberOfLinks))
}
fn final_path(file: &File) -> Result<PathBuf> {
    let mut buffer = vec![0u16; 1024];
    loop {
        let length = unsafe { GetFinalPathNameByHandleW(file.as_raw_handle() as _,
            buffer.as_mut_ptr(), buffer.len() as u32, 0) } as usize;
        if length == 0 { return Err(std::io::Error::last_os_error()).context("Resolve pinned ACL object path"); }
        if length < buffer.len() { return Ok(PathBuf::from(OsString::from_wide(&buffer[..length]))); }
        if length > 32760 { bail!("Pinned ACL path exceeds Windows bounds"); }
        buffer.resize(length + 1, 0);
    }
}

struct Pinned {
    file: File, path: PathBuf, identity: Identity, directory: bool, links: u32,
    acl_ready: bool,
}
impl Pinned {
    fn check(&self) -> Result<()> {
        let (id, directory, links) = identity(&self.file)?;
        if id != self.identity || directory != self.directory || (!directory && links != self.links)
            || !equal_path(&final_path(&self.file)?, &self.path)? {
            bail!("ACL lease object changed identity, path or link count during preparation");
        }
        Ok(())
    }
}

// No-share-delete handles pin every ancestor while granting/revoking. Targets
// add WRITE_DAC to read-control/attributes/traversal; they never request DELETE
// or data write access. NtSetSecurityObject edits one checked object without
// SetSecurityInfo's user-mode inheritance traversal. This also works when a
// directory is already held as a process CWD, or an executable is mapped.
#[derive(Default)]
struct Pins { entries: BTreeMap<String, Arc<Pinned>> }
fn shared_pins() -> &'static Mutex<BTreeMap<String, Weak<Pinned>>> {
    static PINS: OnceLock<Mutex<BTreeMap<String, Weak<Pinned>>>> = OnceLock::new();
    PINS.get_or_init(|| Mutex::new(BTreeMap::new()))
}
impl Pins {
    fn open(&mut self, requested: &Path, security_file: bool) -> Result<Arc<Pinned>> {
        let mut components = requested.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            bail!("AppContainer ACL roots must use absolute local disk paths");
        };
        let letter = match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => letter,
            _ => bail!("ACL leases do not grant UNC/device/network paths"),
        };
        if !matches!(components.next(), Some(Component::RootDir)) {
            bail!("ACL lease root is not an absolute local disk path");
        }
        let mut current = PathBuf::from(format!(r"\\?\{}:\", (letter as char).to_ascii_uppercase()));
        let parts = components.collect::<Vec<_>>();
        let mut result = self.component(&current, false)?;
        for (index, part) in parts.iter().enumerate() {
            let Component::Normal(name) = part else { bail!("ACL roots may not contain relative traversal"); };
            let text = name.to_str().context("ACL path component must be Unicode")?;
            if text.is_empty() || text.contains(':') || text.ends_with([' ', '.']) {
                bail!("ACL roots may not contain alternate streams or ambiguous components");
            }
            if !result.directory { bail!("ACL root ancestor is not a directory"); }
            current.push(name);
            result = self.component(&current, security_file && index + 1 == parts.len())?;
        }
        Ok(result)
    }
    fn component(&mut self, path: &Path, security_file: bool) -> Result<Arc<Pinned>> {
        let lookup = key(path)?;
        if let Some(existing) = self.entries.get(&lookup) {
            existing.check()?;
            if !security_file || existing.acl_ready { return Ok(existing.clone()); }
        }
        // Concurrent leases share lifetime pins. An ancestor-only handle can
        // safely be upgraded using another no-delete security handle without
        // asking for DELETE or altering any existing sharing restrictions.
        let mut shared = shared_pins().lock().map_err(|_| anyhow::anyhow!("ACL pin registry poisoned"))?;
        if let Some(existing) = shared.get(&lookup).and_then(Weak::upgrade) {
            existing.check()?;
            if !security_file || existing.acl_ready {
                self.entries.insert(lookup, existing.clone());
                return Ok(existing);
            }
        }
        if self.entries.len() > MAX_NODES + MAX_ROOTS * MAX_DEPTH {
            bail!("ACL lease exceeds its bounded object inventory");
        }
        let text = path_wide(path)?;
        let attributes = unsafe { GetFileAttributesW(text.as_ptr()) };
        if attributes == INVALID_FILE_ATTRIBUTES {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("Inspect ACL lease path component {}", path.display()));
        }
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 { bail!("ACL leases refuse reparse path components"); }
        let directory = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        let access = READ_CONTROL | READ_ATTRIBUTES
            | if directory { 0x20 } else { 0 }
            | if security_file { WRITE_DAC } else { 0 };
        let file = OpenOptions::new().access_mode(access)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path).with_context(|| format!(
                "Pin ACL lease object {} without following reparse points (access 0x{access:08x})", path.display()))?;
        let (id, actual_directory, links) = identity(&file)?;
        if actual_directory != directory { bail!("ACL path changed type during handle opening"); }
        let actual = final_path(&file)?;
        if !equal_path(&actual, path)? { bail!("ACL path resolved outside its pinned ancestry"); }
        let value = Arc::new(Pinned { file, path: actual, identity: id, directory, links,
            acl_ready: security_file });
        shared.insert(lookup.clone(), Arc::downgrade(&value));
        self.entries.insert(lookup, value.clone());
        Ok(value)
    }
}

struct Dacl {
    _allocation: LocalAllocation, acl: *mut ACL, owner: *mut c_void,
    control: u16,
}
impl Dacl {
    fn read(file: &File) -> Result<Self> {
        let mut acl = ptr::null_mut();
        let mut owner = ptr::null_mut();
        let mut descriptor = ptr::null_mut();
        let code = unsafe { GetSecurityInfo(file.as_raw_handle() as _, SE_FILE_OBJECT,
            DACL_INFORMATION | 1, &mut owner, ptr::null_mut(), &mut acl,
            ptr::null_mut(), &mut descriptor) };
        if code != 0 { return Err(std::io::Error::from_raw_os_error(code as i32)).context("Read current ACL"); }
        let allocation = LocalAllocation(descriptor);
        let mut control = 0u16;
        let mut revision = 0u32;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Inspect ACL inheritance protection");
        }
        if !acl.is_null() && unsafe { IsValidAcl(acl) } == 0 { bail!("ACL lease encountered an invalid DACL"); }
        Ok(Self { _allocation: allocation, acl, owner, control })
    }
    fn entries(&self) -> Result<Vec<Vec<u8>>> {
        if self.acl.is_null() { return Ok(Vec::new()); }
        let bytes = unsafe { (*self.acl).AclSize } as usize;
        if bytes < size_of::<ACL>() { bail!("Truncated native DACL"); }
        let count = unsafe { (*self.acl).AceCount } as usize;
        let start = self.acl as usize;
        let end = start.checked_add(bytes).context("DACL bounds overflow")?;
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let mut pointer = ptr::null_mut();
            if unsafe { GetAce(self.acl, index as u32, &mut pointer) } == 0 {
                return Err(std::io::Error::last_os_error()).context("Read native ACE");
            }
            let address = pointer as usize;
            if address < start + size_of::<ACL>() || address + 4 > end { bail!("ACE header lies outside DACL"); }
            let header = unsafe { std::slice::from_raw_parts(pointer as *const u8, 4) };
            let length = u16::from_le_bytes([header[2], header[3]]) as usize;
            if length < 4 || address.checked_add(length).is_none_or(|n| n > end) { bail!("ACE body lies outside DACL"); }
            result.push(unsafe { std::slice::from_raw_parts(pointer as *const u8, length) }.to_vec());
        }
        Ok(result)
    }
}
fn ace_sid(ace: &[u8]) -> Result<Option<Sid>> {
    if ace.len() < 4 { bail!("ACE is truncated"); }
    let offset = match ace[0] {
        0 | 1 | 2 | 3 | 9 | 10 | 13 | 14 | 17 => 8,
        5 | 6 | 7 | 8 | 11 | 12 | 15 | 16 => {
            if ace.len() < 12 { bail!("Object ACE is truncated"); }
            let flags = u32::from_le_bytes(ace[8..12].try_into().unwrap());
            if flags & !3 != 0 { bail!("Object ACE has unknown SID layout flags"); }
            12 + if flags & 1 != 0 { 16 } else { 0 } + if flags & 2 != 0 { 16 } else { 0 }
        }
        _ => return Ok(None),
    };
    if offset + 8 > ace.len() { bail!("ACE SID is truncated"); }
    let length = 8usize.saturating_add((ace[offset + 1] as usize) * 4);
    if offset + length > ace.len() { bail!("ACE SID exceeds ACE bounds"); }
    let mut aligned = vec![0u32; length.div_ceil(4)];
    unsafe { ptr::copy_nonoverlapping(ace[offset..].as_ptr(), aligned.as_mut_ptr() as *mut u8, length); }
    let raw = aligned.as_ptr() as *mut c_void;
    if unsafe { IsValidSid(raw) } == 0 { bail!("ACE contains an invalid SID"); }
    Ok(Some(Sid { words: aligned }))
}
fn belongs_to(ace: &[u8], sid: &Sid) -> Result<bool> {
    Ok(ace_sid(ace)?.is_some_and(|value| unsafe { EqualSid(value.raw(), sid.raw()) } != 0))
}

fn acl_mutex() -> &'static Mutex<()> { static LOCK: OnceLock<Mutex<()>> = OnceLock::new(); LOCK.get_or_init(|| Mutex::new(())) }

fn set_object_dacl(file: &File, acl: *mut ACL, control: u16) -> Result<()> {
    let mut descriptor: SECURITY_DESCRIPTOR = unsafe { zeroed() };
    let raw = &mut descriptor as *mut SECURITY_DESCRIPTOR as *mut c_void;
    // The native setter treats AUTO_INHERIT_REQ as an input command, not a
    // stored flag. Preserving AUTO_INHERITED therefore requires both bits in
    // the input descriptor; copying the read-back control alone clears it.
    // This does not run SetSecurityInfo's existing-child propagation walk.
    let input_control = control | if control & DACL_AUTO_INHERITED != 0 {
        DACL_AUTO_INHERIT_REQ
    } else { 0 };
    if unsafe { InitializeSecurityDescriptor(raw, 1) } == 0
        || unsafe { SetSecurityDescriptorDacl(raw, 1, acl, if control & 0x08 != 0 { 1 } else { 0 }) } == 0
        || unsafe { SetSecurityDescriptorControl(raw, 0x1500, input_control & 0x1500) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Build object-scoped ACL security descriptor");
    }
    // The documented native call targets this handle only. In contrast,
    // SetSecurityInfo also maintains inherited ACEs through a user-mode tree
    // walk, which can re-add a revoked parent's ACE or touch unchecked paths.
    let status = unsafe { NtSetSecurityObject(file.as_raw_handle() as _, DACL_INFORMATION, raw) };
    if status < 0 {
        let code = unsafe { RtlNtStatusToDosError(status) };
        return Err(std::io::Error::from_raw_os_error(code as i32))
            .with_context(|| format!("Apply object-scoped ACL (NTSTATUS 0x{:08x})", status as u32));
    }
    Ok(())
}

fn edit_acl(node: &Pinned, sid: &Sid, grant: Option<(Access, bool)>) -> Result<()> {
    node.check()?;
    let current = Dacl::read(&node.file)?;
    if current.acl.is_null() {
        if grant.is_some() { bail!("ACL leases refuse NULL DACLs rather than replacing existing permission policy"); }
        return Ok(());
    }
    let entries = current.entries()?;
    let mut retained = Vec::new();
    let mut ours = 0;
    for entry in entries {
        if belongs_to(&entry, sid)? { ours += 1; } else { retained.push(entry); }
    }
    if grant.is_some() && ours != 0 { bail!("Fresh AppContainer SID already occurs in a requested external ACL"); }
    if grant.is_none() && ours == 0 { return Ok(()); }
    let sid_bytes = unsafe { GetLengthSid(sid.raw()) } as usize;
    let needed = size_of::<ACL>() + retained.iter().map(Vec::len).sum::<usize>()
        + if grant.is_some() { 4 * (8 + sid_bytes) } else { 0 };
    if needed > u16::MAX as usize { bail!("AppContainer grant would exceed the native ACL size bound"); }
    let mut storage = vec![0u32; needed.div_ceil(4)];
    let acl = storage.as_mut_ptr() as *mut ACL;
    if unsafe { InitializeAcl(acl, needed as u32, ACL_REVISION_DS_VALUE) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Initialize merged ACL");
    }
    let flags = if node.directory { INHERIT } else { 0 };
    if let Some((access, root)) = grant {
        let deny = if access == Access::ReadExecute { WRITE_EFFECTS } else { SECURITY_EFFECTS };
        if unsafe { AddAccessDeniedAceEx(acl, ACL_REVISION_DS_VALUE, flags, deny, sid.raw()) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Add scoped AppContainer deny ACE");
        }
        if root && access == Access::ReadWrite && unsafe {
            AddAccessDeniedAceEx(acl, ACL_REVISION_DS_VALUE, 0, DELETE, sid.raw())
        } == 0 { return Err(std::io::Error::last_os_error()).context("Protect authorized root identity"); }
    }
    let mut inserted = false;
    let add_allow = |acl: *mut ACL| -> Result<()> {
        if let Some((access, root)) = grant {
            let mask = match access { Access::ReadExecute => READ_EXECUTE,
                Access::ReadWrite => READ_WRITE & if root { !DELETE } else { u32::MAX } };
            if unsafe { AddAccessAllowedAceEx(acl, ACL_REVISION_DS_VALUE, flags, mask, sid.raw()) } == 0 {
                return Err(std::io::Error::last_os_error()).context("Add scoped AppContainer allow ACE");
            }
            if root && node.directory && access == Access::ReadWrite && unsafe {
                AddAccessAllowedAceEx(acl, ACL_REVISION_DS_VALUE, INHERIT | 0x08, DELETE, sid.raw())
            } == 0 {
                return Err(std::io::Error::last_os_error()).context("Allow deletion of future children without allowing root deletion");
            }
        }
        Ok(())
    };
    for entry in &retained {
        if !inserted && entry[1] & INHERITED != 0 { add_allow(acl)?; inserted = true; }
        if unsafe { AddAce(acl, ACL_REVISION_DS_VALUE, u32::MAX,
            entry.as_ptr() as _, entry.len() as u32) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Preserve unrelated ACE");
        }
    }
    if !inserted { add_allow(acl)?; }
    set_object_dacl(&node.file, acl, current.control)?;
    let after = Dacl::read(&node.file)?;
    if after.control & DACL_CONTROL_BITS != current.control & DACL_CONTROL_BITS {
        bail!("ACL edit changed existing DACL inheritance control flags on {} (before 0x{:04x}, after 0x{:04x})",
            node.path.display(), current.control, after.control);
    }
    let mut remaining = 0;
    let mut unrelated = Vec::new();
    for entry in after.entries()? {
        if belongs_to(&entry, sid)? { remaining += 1; } else { unrelated.push(entry); }
    }
    if unrelated != retained || (grant.is_none() && remaining != 0)
        || (grant.is_some() && remaining < 2) {
        bail!("Native ACL read-back did not preserve unrelated permissions and exact lease scope on {} (grant {}, remaining SID ACEs {}, unrelated ACEs unchanged {})",
            node.path.display(), grant.is_some(), remaining, unrelated == retained);
    }
    node.check()
}

struct Node { pinned: Arc<Pinned>, access: Access, root: bool }
fn effective_access(path: &Path, grants: &[RecordedGrant]) -> Result<Access> {
    let mut selected = None;
    let mut depth = 0;
    for grant in grants {
        let root = Path::new(&grant.path);
        if within(path, root)? {
            let candidate_depth = root.components().count();
            if candidate_depth >= depth { selected = Some(grant.access); depth = candidate_depth; }
        }
    }
    // Policy supplies explicit nested grants (for example .git ReadExecute
    // within a ReadWrite project). The narrowest authorized root wins, and
    // edit_acl adds an explicit deny for the effective read-only scope.
    selected.context("Enumerated ACL object escaped its authorized roots")
}

fn inventory(pins: &mut Pins, grants: &[RecordedGrant], validate_links: bool) -> Result<Vec<Node>> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    fn walk(pins: &mut Pins, grants: &[RecordedGrant], path: &Path,
        depth: usize, seen: &mut BTreeSet<String>, result: &mut Vec<Node>) -> Result<()> {
        if depth > MAX_DEPTH || result.len() >= MAX_NODES { bail!("ACL lease tree exceeds configured implementation bounds"); }
        let pinned = pins.open(path, true)?;
        if !seen.insert(key(&pinned.path)?) { return Ok(()); }
        let access = effective_access(&pinned.path, grants)?;
        let root = grants.iter().any(|grant| equal_path(&pinned.path, Path::new(&grant.path)).unwrap_or(false));
        let directory = pinned.directory;
        result.push(Node { pinned: pinned.clone(), access, root });
        if directory {
            for entry in fs::read_dir(&pinned.path).context("Enumerate pinned ACL directory")? {
                let entry = entry?;
                walk(pins, grants, &pinned.path.join(entry.file_name()), depth + 1, seen, result)?;
            }
        }
        Ok(())
    }
    for grant in grants {
        let nested = grants.iter().any(|other| other.path != grant.path
            && within(Path::new(&grant.path), Path::new(&other.path)).unwrap_or(false));
        if !nested { walk(pins, grants, Path::new(&grant.path), 0, &mut seen, &mut result)?; }
    }
    if validate_links {
        let mut files: BTreeMap<Identity, (u32, usize, Access)> = BTreeMap::new();
        for node in &result {
            node.pinned.check()?;
            if node.pinned.directory { continue; }
            let entry = files.entry(node.pinned.identity).or_insert((node.pinned.links, 0, node.access));
            if entry.0 != node.pinned.links || entry.2 != node.access {
                bail!("Hard-linked ACL objects cross read-only/read-write authorization boundaries");
            }
            entry.1 += 1;
        }
        if files.values().any(|(links, names, _)| *links == 0 || *links as usize != *names) {
            bail!("ACL grant refused a hard link whose aliases are outside the authorized inventory");
        }
    }
    Ok(result)
}

fn user_profile() -> Result<PathBuf> {
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Inspect current user's profile boundary");
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let mut length = 0u32;
    unsafe { GetUserProfileDirectoryW(token.as_raw_handle() as _, ptr::null_mut(), &mut length); }
    if length == 0 || length > 32760 { bail!("Windows returned an invalid user-profile path length"); }
    let mut buffer = vec![0u16; length as usize];
    if unsafe { GetUserProfileDirectoryW(token.as_raw_handle() as _, buffer.as_mut_ptr(), &mut length) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Read native user-profile boundary");
    }
    let end = buffer.iter().position(|c| *c == 0).context("Unterminated native user-profile path")?;
    Ok(PathBuf::from(OsString::from_wide(&buffer[..end])))
}

/// Keep the private profile and its immutable tool directory pinned until the
/// complete Job exits. These are disposable profile resources, not external
/// ACL grants: the existing protected profile marker owns their recovery.
/// Drop this guard before DeleteAppContainerProfile, after confirmed Job exit.
pub(super) struct PrivateToolsSeal { _pins: Pins }

fn appcontainer_directory(sid: &str) -> Result<PathBuf> {
    let text = wide(sid);
    let mut raw = ptr::null_mut();
    let result = unsafe { GetAppContainerFolderPath(text.as_ptr(), &mut raw) };
    let path = (|| -> Result<PathBuf> {
        if result < 0 {
            bail!("Resolve private tool profile failed (HRESULT 0x{:08x})", result as u32);
        }
        if raw.is_null() { bail!("Windows returned no private AppContainer profile path"); }
        let mut length = 0usize;
        while length < 32768 && unsafe { *raw.add(length) } != 0 { length += 1; }
        if length == 32768 { bail!("Windows returned an oversized AppContainer profile path"); }
        let path = PathBuf::from(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(raw, length)
        }));
        if !path.is_absolute() { bail!("The native AppContainer profile path must be absolute"); }
        Ok(path)
    })();
    if !raw.is_null() { unsafe { CoTaskMemFree(raw as _); } }
    path
}

fn clipped_file_rights(mut mask: u32, limit: u32) -> u32 {
    let mut mapping = GENERIC_MAPPING { GenericRead: FILE_GENERIC_READ,
        GenericWrite: FILE_GENERIC_WRITE, GenericExecute: FILE_GENERIC_EXECUTE,
        GenericAll: FILE_ALL_ACCESS };
    unsafe { MapGenericMask(&mut mask, &mut mapping); }
    mask & limit
}

/// Restrict the actual package ALLOW rights. A package DENY must never be used
/// to cancel a retained full bootstrap ALLOW. Other principals keep their ACEs
/// unchanged. This helper is used only for this task's disposable native profile;
/// external leases continue to reject pre-existing package ACEs entirely.
fn restrict_private_acl(node: &Pinned, sid: &Sid, profile_root: bool) -> Result<()> {
    node.check()?;
    let current = Dacl::read(&node.file)?;
    if current.acl.is_null() { bail!("Private tools refuse a NULL DACL"); }
    let original = current.entries()?;
    let mut entries = Vec::with_capacity(original.len() + 2);
    let limit = if profile_root { READ_WRITE & !DELETE } else { READ_EXECUTE };
    for entry in &original {
        if !matches!(entry[0], 0 | 1 | 5 | 6 | 9 | 10 | 11 | 12) {
            bail!("Private profile has an unsupported discretionary ACE type {}", entry[0]);
        }
        let allowed = matches!(entry[0], 0 | 5 | 9 | 11);
        if !allowed || !belongs_to(entry, sid)? || (profile_root && entry[1] & 0x08 != 0) {
            entries.push(entry.clone());
            continue;
        }
        if entry.len() < 8 { bail!("Private profile ALLOW ACE is truncated"); }
        let mask = u32::from_le_bytes(entry[4..8].try_into().unwrap());
        let mut effective = entry.clone();
        effective[4..8].copy_from_slice(&clipped_file_rights(mask, limit).to_le_bytes());
        if profile_root && entry[1] & INHERIT as u8 != 0 {
            // Preserve bootstrap inheritance for future Temp/cache siblings,
            // while removing those full rights from the profile root itself.
            effective[1] &= !(INHERIT as u8 | 0x04 | 0x08);
            entries.push(effective);
            let mut future_children = entry.clone();
            future_children[1] |= 0x08;
            entries.push(future_children);
        } else {
            entries.push(effective);
        }
    }
    let sid_bytes = unsafe { GetLengthSid(sid.raw()) } as usize;
    let ace_bytes = 8 + sid_bytes;
    let flags = if !profile_root && node.directory { INHERIT } else { 0 };
    let mask = if profile_root { SECURITY_EFFECTS | DELETE } else { WRITE_EFFECTS };
    let mut deny = vec![1, flags as u8];
    deny.extend_from_slice(&(ace_bytes as u16).to_le_bytes());
    deny.extend_from_slice(&mask.to_le_bytes());
    deny.extend_from_slice(unsafe { std::slice::from_raw_parts(sid.raw() as *const u8, sid_bytes) });
    if entries.first() != Some(&deny) { entries.insert(0, deny); }
    // No higher writable package directory may re-propagate its bootstrap full
    // ALLOW into AC/tools. Root pins alone would not block parent WRITE_DAC.
    let control = current.control | if profile_root { DACL_PROTECTED } else { 0 };
    if entries == original && control == current.control { return node.check(); }
    let needed = size_of::<ACL>() + entries.iter().map(Vec::len).sum::<usize>();
    if needed > u16::MAX as usize { bail!("Private tool deny would exceed the native ACL bound"); }
    let mut storage = vec![0u32; needed.div_ceil(4)];
    let acl = storage.as_mut_ptr() as *mut ACL;
    if unsafe { InitializeAcl(acl, needed as u32, ACL_REVISION_DS_VALUE) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Build bounded private tool ACL");
    }
    for entry in &entries {
        if unsafe { AddAce(acl, ACL_REVISION_DS_VALUE, u32::MAX,
            entry.as_ptr() as _, entry.len() as u32) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Merge restricted private profile ACE");
        }
    }
    set_object_dacl(&node.file, acl, control)?;
    let after = Dacl::read(&node.file)?;
    if after.control & DACL_CONTROL_BITS != control & DACL_CONTROL_BITS
        || after.entries()? != entries {
        bail!("Private tool ACL read-back did not preserve unrelated permissions and exact package ALLOW limits on {} (expected control 0x{:04x}, actual 0x{:04x})",
            node.path.display(), control, after.control);
    }
    node.check()
}

#[cfg(test)]
pub(super) fn private_acl_snapshot(path: &Path) -> Result<String> {
    let mut pins = Pins::default();
    let node = pins.open(path, false)?;
    let dacl = Dacl::read(&node.file)?;
    let entries = dacl.entries()?.into_iter().map(|entry| -> Result<serde_json::Value> {
        Ok(serde_json::json!({"type":entry[0],"flags":entry[1],
            "mask":if entry.len() >= 8 { u32::from_le_bytes(entry[4..8].try_into().unwrap()) } else { 0 },
            "sid":ace_sid(&entry)?.map(|sid|sid_text(sid.raw())).transpose()?}))
    }).collect::<Result<Vec<_>>>()?;
    Ok(serde_json::json!({"path":node.path,"control":dacl.control,
        "owner":sid_text(dacl.owner)?,"aces":entries}).to_string())
}

/// Seal only the fixed `tools` child of this SID's native private profile.
/// The caller must own a newly created Identity with its durable `prepared`
/// record and must not have launched a process. Copy all tools first; then keep
/// the returned guard until the complete Job exits. Failure touches no external
/// resource and the caller must delete its own profile using the normal marker.
///
/// The protected profile root's effective package ALLOW loses security editing,
/// DELETE and DELETE_CHILD, closing parent deletion and inheritance bypasses.
/// Separate inherit-only ACEs retain Temp/cache bootstrap rights. Every package
/// ALLOW in the tools tree, including inherited/IO ACEs, is limited to read/execute.
pub(super) fn seal_private_tools(profile: &Path, sid: &str) -> Result<PrivateToolsSeal> {
    let _guard = acl_mutex().lock().map_err(|_| anyhow::anyhow!("ACL lease coordinator poisoned"))?;
    let sid_value = Sid::parse(sid)?;
    let native = appcontainer_directory(sid)?;
    if !equal_path(profile, &native)? {
        bail!("Private tool sealing requires the exact native profile for its package SID");
    }
    let mut roots = Pins::default();
    let profile = roots.open(profile, true)?;
    if !profile.directory || !equal_path(&profile.path, &native)? {
        bail!("Private tools require the pinned native profile directory");
    }
    let tools = roots.open(&profile.path.join("tools"), true)?;
    if !tools.directory { bail!("The private tools root must be a directory"); }
    let grants = [RecordedGrant {
        path: tools.path.to_str().context("Private tools path must be Unicode")?.to_owned(),
        access: Access::ReadExecute, identity: tools.identity, directory: true,
    }];
    let mut all = Pins { entries: roots.entries.clone() };
    let nodes = inventory(&mut all, &grants, true)?;
    for node in &nodes {
        if !node.pinned.directory && node.pinned.links != 1 {
            bail!("Private tool copies must be independent single-link files");
        }
        let dacl = Dacl::read(&node.pinned.file)?;
        if dacl.acl.is_null() { bail!("Private tool copies must have explicit non-NULL DACLs"); }
        dacl.entries()?;
    }
    restrict_private_acl(&profile, &sid_value, true)?;
    for node in &nodes { restrict_private_acl(&node.pinned, &sid_value, false)?; }
    // Existing children are protected by their native DACL, not by keeping
    // hundreds of thousands of file handles open. Only roots remain pinned.
    drop(nodes);
    drop(all);
    Ok(PrivateToolsSeal { _pins: roots })
}

fn sid_text(raw: *mut c_void) -> Result<String> {
    if raw.is_null() || unsafe { IsValidSid(raw) } == 0 { bail!("Journal owner/ACE has an invalid SID"); }
    let mut text = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(raw, &mut text) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Render journal security principal");
    }
    let _memory = LocalAllocation(text as _);
    let mut length = 0;
    while length < 184 && unsafe { *text.add(length) } != 0 { length += 1; }
    if length == 184 { bail!("Journal security principal SID is too long"); }
    Ok(String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })?)
}
fn current_user_sid() -> Result<String> {
    let mut raw = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Read service journal owner identity");
    }
    let token = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    let mut bytes = 0u32;
    unsafe { GetTokenInformation(raw, TokenUser, ptr::null_mut(), 0, &mut bytes); }
    if bytes < size_of::<TOKEN_USER>() as u32 || bytes > 16_384 {
        bail!("Windows returned invalid service user information bounds");
    }
    let mut storage = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
    if unsafe { GetTokenInformation(token.as_raw_handle() as _, TokenUser,
        storage.as_mut_ptr() as _, bytes, &mut bytes) } == 0 {
        return Err(std::io::Error::last_os_error()).context("Read current service user SID");
    }
    let user = unsafe { &*(storage.as_ptr() as *const TOKEN_USER) };
    sid_text(user.User.Sid)
}

/// Native journal handling shared by ACL and profile recovery. Only a newly
/// created service-specific directory receives a private DACL. Existing host
/// directories and files are validated without changing their permissions.
pub(crate) struct PrivateJournal {
    _pins: Pins, directory: Arc<Pinned>, user: String,
}
impl PrivateJournal {
    pub(crate) fn open(directory: &Path) -> Result<Self> {
        let user = current_user_sid()?;
        let parent = directory.parent().context("Journal requires a private service parent directory")?;
        let mut pins = Pins::default();
        let parent_pin = pins.open(parent, false)?;
        if !parent_pin.directory { bail!("Journal parent is not a directory"); }
        // Passing an explicit protected DACL at creation avoids a writable
        // interval between inheriting an ACL and subsequently hardening it.
        let sddl = wide(&format!("O:{user}D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"));
        let mut descriptor = ptr::null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(), 1, &mut descriptor, ptr::null_mut()) } == 0 {
            return Err(std::io::Error::last_os_error()).context("Build private service journal permissions");
        }
        let _descriptor = LocalAllocation(descriptor);
        let attributes = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
        let native_path = path_wide(directory)?;
        if unsafe { CreateDirectoryW(native_path.as_ptr(), &attributes) } == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(183) { return Err(error).context("Create service-specific journal directory"); }
        }
        let pinned = pins.open(directory, false)?;
        if !pinned.directory { bail!("Service journal must be a real directory"); }
        let result = Self { _pins: pins, directory: pinned, user };
        result.validate(&result.directory.file, true)?;
        Ok(result)
    }
    fn trusted(&self, sid: &str) -> bool {
        sid == self.user || sid == "S-1-5-18" || sid == "S-1-5-32-544"
    }
    fn validate(&self, file: &File, directory: bool) -> Result<()> {
        let path = final_path(file)?;
        let dacl = Dacl::read(file)?;
        let owner = sid_text(dacl.owner)?;
        if dacl.acl.is_null() || !self.trusted(&owner) {
            bail!("Service journal {} must have a trusted owner and a non-NULL DACL (owner {owner}, NULL DACL {})",
                path.display(), dacl.acl.is_null());
        }
        for (index, entry) in dacl.entries()?.into_iter().enumerate() {
            // A deny is never authority to alter a record. Unknown allow ACE
            // formats fail closed rather than being silently ignored.
            if matches!(entry[0], 1 | 6 | 10 | 12) { continue; }
            if !matches!(entry[0], 0 | 5 | 9 | 11) || entry.len() < 8 {
                bail!("Service journal {} has unsupported discretionary ACE #{index} (type {}, flags 0x{:02x})",
                    path.display(), entry[0], entry[1]);
            }
            let inheritable = directory && entry[1] & INHERIT as u8 != 0;
            if entry[1] & 0x08 != 0 && !inheritable { continue; }
            let mask = u32::from_le_bytes(entry[4..8].try_into().unwrap());
            if mask & (WRITE_EFFECTS | 0x5000_0000 | MAXIMUM_ALLOWED) == 0 { continue; }
            let sid = ace_sid(&entry)?.context("Journal write ACE has no recognized SID")?;
            let text = sid_text(sid.raw())?;
            // CREATOR_OWNER is a safe inheritable placeholder: journal files
            // are created only by this service beneath the pinned directory.
            let creator_owner = directory && entry[1] & 0x08 != 0 && text == "S-1-3-0";
            let owner_rights = text == "S-1-3-4";
            if !self.trusted(&text) && !creator_owner && !owner_rights {
                bail!("Service journal {} grants write or security-edit rights to untrusted SID {text} (ACE #{index}, type {}, flags 0x{:02x}, mask 0x{mask:08x})",
                    path.display(), entry[0], entry[1]);
            }
        }
        Ok(())
    }
    fn child(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty() || name.len() > 256 || name.contains(['\\', '/', ':', '\0'])
            || name == "." || name == ".." || name.ends_with([' ', '.']) {
            bail!("Journal entries must use one unambiguous filename component");
        }
        self.directory.check()?;
        self.validate(&self.directory.file, true)?;
        Ok(self.directory.path.join(name))
    }
    pub(crate) fn create(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        let path = self.child(name)?;
        let mut file = OpenOptions::new().write(true).create_new(true)
            .share_mode(FILE_SHARE_READ).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path).context("Create a new exclusive recovery record")?;
        let written = file.write_all(bytes).and_then(|_| file.sync_all());
        drop(file);
        let result = (|| -> Result<()> {
            written.context("Durably write service recovery record")?;
            // Validate the resulting inherited file ACL before creating a
            // profile or granting any external resource to its identity.
            if self.read(name, bytes.len())? != bytes { bail!("New journal record changed during creation"); }
            Ok(())
        })();
        if let Err(error) = result {
            if let Err(cleanup) = self.remove(name) {
                return Err(crate::error::coded("SANDBOX_CLEANUP_PENDING", false,
                    format!("Recovery journal creation failed ({error:#}); partial record could not be removed ({cleanup:#})")));
            }
            return Err(error).context("Recovery record preparation failed; partial record was removed");
        }
        Ok(path)
    }
    pub(crate) fn read(&self, name: &str, max: usize) -> Result<Vec<u8>> {
        let path = self.child(name)?;
        let file = OpenOptions::new().access_mode(READ_CONTROL | READ_ATTRIBUTES | 1)
            .share_mode(FILE_SHARE_READ).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path).context("Open pinned recovery record without write/delete sharing")?;
        let (_, directory, links) = identity(&file)?;
        if directory || links != 1 || !equal_path(&final_path(&file)?, &path)? {
            bail!("Recovery record must be a regular single-link file in its exact journal directory");
        }
        self.validate(&file, false)?;
        if file.metadata()?.len() > max as u64 { bail!("Recovery record exceeds its size bound"); }
        let mut bytes = Vec::new();
        file.take((max as u64).saturating_add(1)).read_to_end(&mut bytes)?;
        if bytes.len() > max { bail!("Recovery record grew beyond its size bound"); }
        Ok(bytes)
    }
    pub(crate) fn entries(&self, max: usize) -> Result<Vec<String>> {
        self.directory.check()?;
        self.validate(&self.directory.file, true)?;
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.directory.path)? {
            let name = entry?.file_name().into_string()
                .map_err(|_| anyhow::anyhow!("Recovery record name is not Unicode"))?;
            self.child(&name)?;
            names.push(name);
            if names.len() > max { bail!("Too many pending service recovery records"); }
        }
        names.sort();
        Ok(names)
    }
    pub(crate) fn remove(&self, name: &str) -> Result<()> {
        fs::remove_file(self.child(name)?).context("Remove completed service recovery record")
    }
}

/// `protected_paths` prohibits all overlaps (normally service state/config).
/// The native HOME root and its ancestors are always prohibited separately;
/// explicitly selected project/toolchain subdirectories below HOME are valid.
pub struct Lease {
    journal: Option<PathBuf>, record: Record, sid: Sid,
    // Keep only roots and their ancestors across execution. Existing child
    // objects can then be renamed/deleted by normal developer commands.
    roots: Pins, journal_directory: Arc<PrivateJournal>, cleanup_on_drop: bool,
}
impl Lease {
    pub fn acquire(journal_root: &Path, sid: &str, grants: &[Grant], protected_paths: &[PathBuf]) -> Result<Self> {
        let _guard = acl_mutex().lock().map_err(|_| anyhow::anyhow!("ACL lease coordinator poisoned"))?;
        shared_pins().lock().map_err(|_| anyhow::anyhow!("ACL pin registry poisoned"))?
            .retain(|_, value| value.strong_count() != 0);
        let sid_value = Sid::parse(sid)?;
        if grants.is_empty() || grants.len() > MAX_ROOTS { bail!("ACL lease requires 1..128 precise authorized roots"); }
        let journal_directory = Arc::new(PrivateJournal::open(journal_root)?);
        let home = user_profile()?;
        let mut roots = Pins::default();
        let mut recorded: Vec<RecordedGrant> = Vec::new();
        let mut ordered_grants = grants.iter().collect::<Vec<_>>();
        ordered_grants.sort_by_key(|grant| grant.path.components().count());
        for grant in ordered_grants {
            let pinned = roots.open(&grant.path, true)?;
            if pinned.path.components().count() <= 2 || within(&home, &pinned.path)?
                || within(journal_root, &pinned.path)? || within(&pinned.path, journal_root)? {
                bail!("ACL grants must not expose a drive root, complete HOME, or the lease journal");
            }
            for protected in protected_paths {
                if within(protected, &pinned.path)? || within(&pinned.path, protected)? {
                    bail!("ACL grant overlaps protected service state/configuration");
                }
            }
            if let Some(existing) = recorded.iter().find(|entry| equal_path(Path::new(&entry.path), &pinned.path).unwrap_or(false)) {
                if existing.access != grant.access { bail!("Conflicting grants for the same ACL root"); }
                continue;
            }
            recorded.push(RecordedGrant { path: pinned.path.to_str().context("ACL root must be Unicode")?.to_owned(),
                access: grant.access, identity: pinned.identity, directory: pinned.directory });
        }
        // The inventory shares the authorized roots' lifetime pins.
        let mut all = Pins { entries: roots.entries.clone() };
        let nodes = inventory(&mut all, &recorded, true)?;
        for node in &nodes {
            let dacl = Dacl::read(&node.pinned.file)?;
            if dacl.acl.is_null() { bail!("ACL leases refuse NULL DACLs"); }
            for entry in dacl.entries()? { if belongs_to(&entry, &sid_value)? {
                bail!("Fresh lease SID already appears in an external ACL");
            } }
        }
        let nonce = crate::util::digest(crate::util::random_secret()?);
        let lease_id = nonce[..32].to_owned();
        let record = Record { version: 1, lease: lease_id.clone(), sid: sid.to_owned(), grants: recorded };
        let journal_name = format!("lease-{lease_id}.json");
        let bytes = serde_json::to_vec(&record)?;
        if bytes.len() > MAX_JOURNAL_BYTES { bail!("ACL lease journal exceeds its size bound"); }
        let journal = journal_directory.create(&journal_name, &bytes)
            .context("Journal ACL lease before changing host ACLs")?;
        let mut lease = Self { journal: Some(journal), record, sid: sid_value, roots,
            journal_directory, cleanup_on_drop: true };
        let mut edited = BTreeSet::new();
        let result = (|| -> Result<()> {
            for node in &nodes {
                // Multiple in-tree hard links share one DACL. Mutate it once.
                if edited.insert(node.pinned.identity) {
                    edit_acl(&node.pinned, &lease.sid, Some((node.access, node.root)))?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            lease.cleanup_on_drop = false;
            // The original checked inventory remains pinned during rollback.
            if let Err(cleanup) = revoke_nodes(&nodes, &lease.sid) {
                return Err(crate::error::coded("SANDBOX_CLEANUP_PENDING", false,
                    format!("ACL lease preparation failed ({error:#}); permission revocation requires service recovery ({cleanup:#})")));
            }
            if let Err(cleanup) = lease.journal_directory.remove(&journal_name) {
                return Err(crate::error::coded("SANDBOX_CLEANUP_PENDING", false,
                    format!("ACL lease preparation failed ({error:#}); permissions were revoked but its recovery record could not be removed ({cleanup:#})")));
            }
            lease.journal = None;
            return Err(error).context("ACL lease preparation failed; added permissions were revoked");
        }
        drop(nodes);
        drop(all);
        Ok(lease)
    }
    /// Call only after the entire Job Object is empty and its handles are closed.
    pub fn finish(&mut self) -> Result<()> {
        let _guard = acl_mutex().lock().map_err(|_| anyhow::anyhow!("ACL lease coordinator poisoned"))?;
        self.finish_locked()
    }
    fn finish_locked(&mut self) -> Result<()> {
        let Some(path) = self.journal.clone() else { return Ok(()); };
        for grant in &self.record.grants {
            let pinned = self.roots.open(Path::new(&grant.path), true)?;
            pinned.check()?;
            if pinned.identity != grant.identity || pinned.directory != grant.directory {
                bail!("ACL lease root changed identity; recovery evidence retained");
            }
        }
        let mut pins = Pins { entries: self.roots.entries.clone() };
        let nodes = inventory(&mut pins, &self.record.grants, false)?;
        revoke_nodes(&nodes, &self.sid)?;
        let name = path.file_name().and_then(|value| value.to_str()).context("Invalid ACL journal filename")?;
        self.journal_directory.remove(name).context("Clear fully revoked ACL lease journal")?;
        self.journal = None;
        Ok(())
    }
    /// If contained processes might still be alive, revocation/profile deletion
    /// must wait for exclusive-startup recovery, not race those processes.
    pub fn defer_cleanup(&mut self) { self.cleanup_on_drop = false; }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.cleanup_on_drop && self.journal.is_some() {
            if let Err(error) = self.finish() { tracing::error!(error=%error, "AppContainer ACL cleanup deferred to protected recovery"); }
        }
    }
}
fn revoke_nodes(nodes: &[Node], sid: &Sid) -> Result<()> {
    let mut seen = BTreeSet::new();
    let mut first_error = None;
    for node in nodes.iter().rev() {
        if seen.insert(node.pinned.identity) {
            if let Err(error) = edit_acl(&node.pinned, sid, None) { if first_error.is_none() { first_error = Some(error); } }
        }
    }
    if let Some(error) = first_error { Err(error).context("Could not revoke every AppContainer ACE; journal retained") } else { Ok(()) }
}

fn valid_lease(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn read_lease_record(root: &PrivateJournal, entry: &str) -> Result<Record> {
    let id = entry.strip_prefix("lease-").and_then(|s| s.strip_suffix(".json"))
        .filter(|id| valid_lease(id)).context("Unrecognized ACL recovery journal name")?;
    let bytes = root.read(entry, MAX_JOURNAL_BYTES)?;
    let record: Record = serde_json::from_slice(&bytes).context("Malformed protected ACL recovery journal")?;
    if record.version != 1 || record.lease != id || record.grants.is_empty() || record.grants.len() > MAX_ROOTS {
        bail!("ACL recovery journal version, identity or grant count is invalid");
    }
    Sid::parse(&record.sid)?;
    if record.grants.iter().any(|grant| grant.identity.volume == 0 || grant.identity.index == 0) {
        bail!("ACL recovery journal contains an invalid file identity");
    }
    Ok(record)
}
/// Read-only preflight for the execution-exit gate. All protected records are
/// validated before returning any SID. Authorization and live root FileIDs are
/// still checked by recover_pending before its first ACL mutation.
pub fn pending_sids(journal_root: &Path) -> Result<BTreeSet<String>> {
    match fs::symlink_metadata(journal_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error).context("Inspect pending ACL journal"),
        Ok(_) => {},
    }
    let _guard = acl_mutex().lock().map_err(|_| anyhow::anyhow!("ACL lease coordinator poisoned"))?;
    let root = PrivateJournal::open(journal_root)?;
    let mut sids = BTreeSet::new();
    for entry in root.entries(MAX_PENDING)? { sids.insert(read_lease_record(&root, &entry)?.sid); }
    Ok(sids)
}
/// Caller must hold the exclusive service instance lock, before profiles are
/// deleted or any new Job is admitted. Recovery only removes recorded SID ACEs.
pub fn recover_pending(journal_root: &Path, allowed_roots: &[PathBuf]) -> Result<usize> {
    let _guard = acl_mutex().lock().map_err(|_| anyhow::anyhow!("ACL lease coordinator poisoned"))?;
    let root = Arc::new(PrivateJournal::open(journal_root)?);
    let entries = root.entries(MAX_PENDING)?;
    let mut verified = Vec::new();
    for entry in entries {
        let record = read_lease_record(&root, &entry)?;
        let sid = Sid::parse(&record.sid)?;
        let mut pins = Pins::default();
        let mut seen = BTreeSet::new();
        let mut ordered_grants = record.grants.iter().collect::<Vec<_>>();
        ordered_grants.sort_by_key(|grant| Path::new(&grant.path).components().count());
        for grant in ordered_grants {
            let path = Path::new(&grant.path);
            if !allowed_roots.iter().any(|allowed| within(path, allowed).unwrap_or(false))
                || !seen.insert(key(path)?) { bail!("ACL journal root is outside current authorization or duplicated"); }
            let pinned = pins.open(path, true)?;
            if pinned.identity != grant.identity || pinned.directory != grant.directory {
                bail!("ACL recovery root no longer matches its recorded file identity");
            }
        }
        verified.push(Lease { journal: Some(journal_root.join(entry)), record, sid,
            roots: pins, journal_directory: root.clone(), cleanup_on_drop: false });
    }
    // Validation above is read-only for the complete batch. Never revoke a
    // valid lease merely because a later record proved corrupt or unauthorized.
    let count = verified.len();
    for lease in &mut verified { lease.finish_locked()?; }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sid(n: u32) -> String { format!("S-1-15-2-31-41-59-26-53-58-{n}") }
    fn count_sid(path: &Path, text: &str) -> usize {
        let mut pins = Pins::default();
        let file = pins.open(path, true).unwrap();
        let sid = Sid::parse(text).unwrap();
        Dacl::read(&file.file).unwrap().entries().unwrap().iter()
            .filter(|ace| belongs_to(ace, &sid).unwrap()).count()
    }
    fn set_test_dacl(file: &File, sddl: &str) {
        use windows_sys::Win32::Security::GetSecurityDescriptorDacl;
        let text = wide(sddl);
        let mut descriptor = ptr::null_mut();
        assert_ne!(unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(), 1, &mut descriptor, ptr::null_mut()) }, 0);
        let _allocation = LocalAllocation(descriptor);
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        assert_ne!(unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }, 0);
        assert_ne!(present, 0);
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }, 0);
        set_object_dacl(file, acl, control).unwrap();
    }
    #[test]
    fn rejects_capability_sids_and_unsafe_lease_names() {
        assert!(Sid::parse("S-1-15-2-1").is_err());
        assert!(Sid::parse("S-1-15-3-1").is_err());
        assert!(Sid::parse(&sid(1)).is_ok());
        assert!(!valid_lease("../../other"));
    }
    #[test]
    fn pending_sid_preflight_validates_all_records_without_revoking() {
        let temp = tempfile::tempdir().unwrap();
        let journal = temp.path().join("journal");
        assert!(pending_sids(&journal).unwrap().is_empty());
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let text = sid(99);
        let mut lease = Lease::acquire(&journal, &text,
            &[Grant { path: project.clone(), access: Access::ReadWrite }], &[]).unwrap();
        assert_eq!(pending_sids(&journal).unwrap(), BTreeSet::from([text.clone()]));
        let invalid = journal.join("unexpected.json");
        fs::write(&invalid, b"untrusted content").unwrap();
        assert!(pending_sids(&journal).is_err());
        assert!(count_sid(&project, &text) > 0);
        fs::remove_file(invalid).unwrap();
        lease.finish().unwrap();
    }
    #[test]
    fn leases_restore_only_their_sid_and_keep_new_files() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let journal = temp.path().join("journal");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("source.rs"), b"source").unwrap();
        let a = sid(101);
        let b = sid(102);
        let grants = [Grant { path: project.clone(), access: Access::ReadWrite }];
        let mut first = Lease::acquire(&journal, &a, &grants, &[]).unwrap();
        let mut second = Lease::acquire(&journal, &b, &grants, &[]).unwrap();
        fs::write(project.join("generated.txt"), b"keep").unwrap();
        first.finish().unwrap();
        assert_eq!(count_sid(&project.join("generated.txt"), &a), 0);
        assert!(count_sid(&project.join("generated.txt"), &b) > 0);
        second.finish().unwrap();
        assert_eq!(count_sid(&project.join("source.rs"), &b), 0);
        assert_eq!(fs::read(project.join("generated.txt")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&journal).unwrap().count(), 0);
    }
    #[test]
    fn external_hard_links_and_protected_overlap_fail_before_grant() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("data"), b"data").unwrap();
        fs::hard_link(project.join("data"), temp.path().join("outside")).unwrap();
        let journal = temp.path().join("journal");
        let grants = [Grant { path: project.clone(), access: Access::ReadWrite }];
        assert!(Lease::acquire(&journal, &sid(201), &grants, &[]).is_err());
        fs::remove_file(temp.path().join("outside")).unwrap();
        assert!(Lease::acquire(&journal, &sid(202), &grants, &[project.join("data")]).is_err());
        assert_eq!(count_sid(&project, &sid(201)), 0);
    }
    #[test]
    fn orphan_recovery_is_bounded_authorized_and_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let journal = temp.path().join("journal");
        let text = sid(301);
        let mut lease = Lease::acquire(&journal, &text,
            &[Grant { path: project.clone(), access: Access::ReadExecute }], &[]).unwrap();
        lease.defer_cleanup();
        drop(lease);
        assert!(recover_pending(&journal, &[]).is_err());
        assert!(count_sid(&project, &text) > 0);
        assert_eq!(recover_pending(&journal, &[project.clone()]).unwrap(), 1);
        assert_eq!(count_sid(&project, &text), 0);
        assert_eq!(recover_pending(&journal, &[project]).unwrap(), 0);
    }
    #[test]
    fn nested_readonly_grants_preserve_protected_children_and_inherited_denies() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let git = project.join(".git");
        let objects = git.join("objects");
        fs::create_dir_all(&objects).unwrap();
        let file = objects.join("existing");
        fs::write(&file, b"git object").unwrap();
        let mut pins = Pins::default();
        let objects_pin = pins.open(&objects, true).unwrap();
        let user = current_user_sid().unwrap();
        set_test_dacl(&objects_pin.file, &format!("D:P(A;OICI;FA;;;{user})"));
        let before = Dacl::read(&objects_pin.file).unwrap().entries().unwrap();
        drop(pins);
        let text = sid(401);
        let mut lease = Lease::acquire(&temp.path().join("journal"), &text, &[
            Grant { path: project, access: Access::ReadWrite },
            Grant { path: git.clone(), access: Access::ReadExecute },
        ], &[]).unwrap();
        let mut pins = Pins::default();
        let sid = Sid::parse(&text).unwrap();
        for path in [&git, &objects, &file] {
            let pinned = pins.open(path, true).unwrap();
            let entries = Dacl::read(&pinned.file).unwrap().entries().unwrap();
            let own = entries.iter().filter(|ace| belongs_to(ace, &sid).unwrap()).collect::<Vec<_>>();
            assert!(own.iter().any(|ace| ace[0] == 1 && ace[1] & INHERITED == 0
                && u32::from_le_bytes(ace[4..8].try_into().unwrap()) & WRITE_EFFECTS == WRITE_EFFECTS));
            assert!(!own.iter().any(|ace| ace[0] == 0
                && u32::from_le_bytes(ace[4..8].try_into().unwrap()) & WRITE_EFFECTS != 0));
        }
        let created = git.join("new-host-file");
        fs::write(&created, b"must inherit read-only sandbox permission").unwrap();
        let pinned = pins.open(&created, true).unwrap();
        assert!(Dacl::read(&pinned.file).unwrap().entries().unwrap().iter().any(|ace|
            belongs_to(ace, &sid).unwrap() && ace[0] == 1 && ace[1] & INHERITED != 0));
        lease.finish().unwrap();
        let after = Dacl::read(&objects_pin.file).unwrap();
        assert_ne!(after.control & DACL_PROTECTED, 0);
        assert_eq!(after.entries().unwrap(), before);
        assert_eq!(count_sid(&created, &text), 0);
    }
    #[test]
    fn in_tree_hard_links_are_granted_and_revoked_once() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let first = project.join("one");
        let second = project.join("two");
        fs::write(&first, b"same artifact").unwrap();
        fs::hard_link(&first, &second).unwrap();
        let text = sid(402);
        let mut lease = Lease::acquire(&temp.path().join("journal"), &text,
            &[Grant { path: project, access: Access::ReadWrite }], &[]).unwrap();
        assert!(count_sid(&first, &text) > 0);
        lease.finish().unwrap();
        assert_eq!(count_sid(&first, &text), 0);
        assert_eq!(count_sid(&second, &text), 0);
    }
    #[test]
    fn leases_support_a_root_held_as_a_process_current_directory() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("source.txt"), b"one bounded fixture file").unwrap();
        // Model the relevant CWD share contract without changing this test
        // process's global working directory or touching the real repository.
        let _cwd = OpenOptions::new().access_mode(READ_CONTROL | READ_ATTRIBUTES | 0x20)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&project).unwrap();
        let mut lease = Lease::acquire(&temp.path().join("journal"), &sid(403),
            &[Grant { path: project.clone(), access: Access::ReadWrite }], &[])
            .unwrap_or_else(|error| panic!("An authorized Project held as CWD must remain usable: {error:#}"));
        lease.finish().unwrap();
        assert_eq!(count_sid(&project, &sid(403)), 0);
    }
    #[test]
    fn handle_acl_edits_do_not_propagate_to_existing_children() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let existing_directory = project.join("existing");
        fs::create_dir_all(&existing_directory).unwrap();
        let existing_file = existing_directory.join("file.txt");
        fs::write(&existing_file, b"untouched existing child").unwrap();
        let mut pins = Pins::default();
        let root = pins.open(&project, true).unwrap();
        let child = pins.open(&existing_directory, true).unwrap();
        let file = pins.open(&existing_file, true).unwrap();
        let child_before = Dacl::read(&child.file).unwrap().entries().unwrap();
        let file_before = Dacl::read(&file.file).unwrap().entries().unwrap();
        let text = sid(404);
        let sid = Sid::parse(&text).unwrap();
        edit_acl(&root, &sid, Some((Access::ReadWrite, true))).unwrap();
        assert_eq!(Dacl::read(&child.file).unwrap().entries().unwrap(), child_before);
        assert_eq!(Dacl::read(&file.file).unwrap().entries().unwrap(), file_before);
        let created = project.join("created.txt");
        fs::write(&created, b"new children inherit their scoped lease").unwrap();
        let created = pins.open(&created, true).unwrap();
        assert!(Dacl::read(&created.file).unwrap().entries().unwrap().iter()
            .any(|ace| belongs_to(ace, &sid).unwrap()));
        edit_acl(&root, &sid, None).unwrap();
        // Removing from a parent also must not perform an implicit tree walk.
        // The lease manager explicitly visits and revokes every new object.
        assert!(Dacl::read(&created.file).unwrap().entries().unwrap().iter()
            .any(|ace| belongs_to(ace, &sid).unwrap()));
        edit_acl(&created, &sid, None).unwrap();
        assert_eq!(count_sid(&project, &text), 0);
        assert_eq!(count_sid(&created.path, &text), 0);
    }
    #[test]
    fn private_journals_reject_hardlinks_and_untrusted_write_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("journal");
        let journal = PrivateJournal::open(&directory).unwrap();
        let record = journal.create("entry.pending", b"durable evidence").unwrap();
        assert_eq!(journal.read("entry.pending", 64).unwrap(), b"durable evidence");
        let alias = temp.path().join("alias");
        fs::hard_link(&record, &alias).unwrap();
        assert!(journal.read("entry.pending", 64).is_err());
        fs::remove_file(alias).unwrap();
        let security = OpenOptions::new().access_mode(WRITE_DAC | READ_CONTROL | READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&directory).unwrap();
        set_test_dacl(&security,
            &format!("D:P(A;OICI;FA;;;{})(A;OICI;FW;;;WD)", journal.user));
        assert!(journal.read("entry.pending", 64).is_err());
        assert!(PrivateJournal::open(&directory).is_err());
        assert_eq!(fs::read(record).unwrap(), b"durable evidence");
    }
}
