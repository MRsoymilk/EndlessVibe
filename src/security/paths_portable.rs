//! Capability-based file operations on macOS and Windows.
//! Linux retains the openat2/renameat2 implementation in paths.rs.
//! cap-std resolves child paths relative to a held directory handle, preventing
//! traversal outside the authorized project even when directories are renamed.
use anyhow::{bail, Context, Result};
use cap_std::{ambient_authority, fs::{Dir, OpenOptions}};
use std::{fs::Metadata, io::{Read, Write}, path::{Component, Path, PathBuf}};

pub struct Root { directory: Dir, pub path: PathBuf }
pub struct Entry { pub name: String, pub kind: &'static str, pub bytes: Option<u64> }

pub fn denied_component(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    matches!(value.as_str(),
      ".git" | ".ssh" | ".gnupg" | ".aws" | ".azure" | ".kube" | ".netrc"
      | ".npmrc" | ".pypirc" | ".git-credentials" | "credentials.json"
      | "id_rsa" | "id_ed25519")
      || value == ".env" || value.starts_with(".env.")
      || value.starts_with(".endlessvibe-")
      || [".pem", ".key", ".p12", ".pfx"].iter().any(|suffix| value.ends_with(suffix))
}

fn windows_reserved_component(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or("").trim_end_matches([' ', '.']).to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL"
       | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
       | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9")
       || value.contains(':') || value.ends_with([' ', '.'])
}

pub fn relative(value: &str, allow_root: bool) -> Result<PathBuf> {
    if value.len() > 4096 || value.contains(['\0', '\\', '\n', '\r'])
       || (!allow_root && value.ends_with('/')) {
        bail!("Invalid relative path");
    }
    let path = Path::new(value);
    if value.is_empty() || path == Path::new(".") {
        if allow_root { return Ok(PathBuf::from(".")); }
        bail!("A file path is required");
    }
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let name = part.to_str().context("Non-UTF8 relative path")?;
                if denied_component(name) || windows_reserved_component(name) {
                    bail!("Sensitive, internal or platform-reserved path is not accessible");
                }
            }
            Component::CurDir => {}
            _ => bail!("Only project-relative paths without '..' are accepted")
        }
    }
    if path.file_name().is_none() { bail!("Invalid path"); }
    Ok(path.to_owned())
}
impl Root {
    pub fn same_directory(&self, other: &Self) -> Result<bool> {
        let a = self.directory.try_clone()?.into_std_file();
        let b = other.directory.try_clone()?.into_std_file();
        Ok(crate::platform::same_file(&a, &b))
    }
    pub fn has_single_link(&self, path: &str) -> Result<bool> {
        let path = relative(path, false)?;
        Ok(self.file(&path).is_ok())
    }
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize().context("Resolve workspace root")?;
        if path.parent().is_none() { bail!("Filesystem root cannot be a Workspace"); }
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            bail!("Workspace root must not be a symlink");
        }
        let directory = Dir::open_ambient_dir(&path, ambient_authority())?;
        let metadata = directory.dir_metadata()?;
        if !metadata.is_dir() { bail!("Workspace root is not a directory"); }
        Ok(Self { directory, path })
    }
    pub fn unchanged_root(&self) -> Result<()> {
        let original = self.directory.try_clone()?.into_std_file();
        let current = std::fs::symlink_metadata(&self.path)?;
        if !current.is_dir() || current.file_type().is_symlink() {
            bail!("Workspace root moved or was replaced; reauthorize it");
        }
        // Compare pinned handles: Windows std::fs::Metadata has no stable file ID.
        let current = Dir::open_ambient_dir(&self.path, ambient_authority())?.into_std_file();
        if !crate::platform::same_file(&original, &current) {
            bail!("Workspace root moved or was replaced; reauthorize it");
        }
        Ok(())
    }
    fn check_components(&self, path: &Path, final_may_be_missing: bool) -> Result<()> {
        let mut cumulative=PathBuf::new();
        let components=path.components().filter_map(|component| match component {
            Component::Normal(name) => Some(name), _ => None
        }).collect::<Vec<_>>();
        for (index, component) in components.iter().enumerate() {
            cumulative.push(component);
            let metadata = match self.directory.symlink_metadata(&cumulative) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    && final_may_be_missing && index == components.len()-1 => return Ok(()),
                Err(error) => return Err(error).with_context(||format!("Inspect {}",cumulative.display()))
            };
            if metadata.file_type().is_symlink() {
                bail!("Symbolic links and junctions are blocked by project file APIs");
            }
            if index+1 != components.len() && !metadata.is_dir() {
                bail!("A path parent is not a directory");
            }
        }
        Ok(())
    }
    fn file(&self, path: &Path) -> Result<std::fs::File> {
        self.check_components(path, false)?;
        let f = self.directory.open(path)?.into_std();
        let metadata = f.metadata()?;
        if !metadata.is_file() || !crate::platform::single_link(&f) {
            bail!("Only regular, single-link files are accessible");
        }
        Ok(f)
    }
    pub fn read(&self, path: &str, max: usize) -> Result<Vec<u8>> {
        let path = relative(path, false)?;
        let file = self.file(&path)?;
        if file.metadata()?.len() > max as u64 {
            bail!("File exceeds maximum size ({max} bytes)");
        }
        let mut bytes=Vec::new();
        file.take((max as u64).saturating_add(1)).read_to_end(&mut bytes)?;
        if bytes.len() > max { bail!("File grew beyond size limit"); }
        Ok(bytes)
    }
    pub fn read_optional(&self, path: &str, max: usize) -> Result<Option<Vec<u8>>> {
        match self.read(path, max) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.chain().any(|cause|
                cause.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)) => Ok(None),
            Err(error) => Err(error)
        }
    }
    pub fn metadata(&self, path: &str) -> Result<Metadata> {
        let path = relative(path, true)?;
        if path == Path::new(".") {
            return Ok(self.directory.try_clone()?.into_std_file().metadata()?);
        }
        self.check_components(&path, false)?;
        match self.directory.open_dir(&path) {
            Ok(dir) => Ok(dir.into_std_file().metadata()?),
            Err(_) => Ok(self.directory.open(&path)?.into_std().metadata()?)
        }
    }
    pub fn directory_path(&self, path: &str) -> Result<PathBuf> {
        let path = relative(path, true)?;
        self.check_components(&path, false)?;
        self.directory.open_dir(&path)?;
        self.unchanged_root()?;
        Ok(self.path.join(path))
    }
    pub fn entries(&self, path: &str) -> Result<Vec<Entry>> {
        let path = relative(path, true)?;
        self.check_components(&path, false)?;
        let dir=self.directory.open_dir(&path)?;
        let mut rows=Vec::new();
        for item in dir.read_dir(".")? {
            let item=item?;
            let Some(name)=item.file_name().to_str().map(str::to_owned) else {continue};
            if denied_component(&name) || windows_reserved_component(&name) {continue;}
            let file_type=item.file_type()?;
            let (kind,bytes)=if file_type.is_symlink() {("symlink_blocked",None)}
                else if file_type.is_dir() {("directory",None)}
                else if file_type.is_file() {("file",item.metadata().ok().map(|m|m.len()))}
                else {("special_blocked",None)};
            rows.push(Entry{name,kind,bytes});
            if rows.len()>20_000 {bail!("Directory is too large to list");}
        }
        rows.sort_by(|a,b|a.name.cmp(&b.name));
        Ok(rows)
    }
    pub fn create_directory(&self, path: &str) -> Result<()> {
        let path=relative(path, false)?;
        let mut current=PathBuf::new();
        for component in path.components() {
            if let Component::Normal(name)=component {
                current.push(name);
                self.directory.create_dir(&current).or_else(|error| {
                    if error.kind()==std::io::ErrorKind::AlreadyExists {Ok(())} else {Err(error)}
                })?;
                self.check_components(&current,false)?;
                self.directory.open_dir(&current)?;
            }
        }
        Ok(())
    }
    pub fn replace(&self, path: &str, bytes: &[u8], create_only: bool, create_parents: bool) -> Result<()> {
        let path=relative(path,false)?;
        let parent=path.parent().filter(|p|!p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        if create_parents && parent!=Path::new(".") {
            self.create_directory(parent.to_str().context("Invalid parent")?)?;
        }
        self.check_components(parent,false)?;
        let dir=self.directory.open_dir(parent)?;
        let name=path.file_name().context("Missing file name")?;
        let previous=if create_only {
            if dir.symlink_metadata(name).is_ok() {bail!("Target already exists");}
            None
        } else {
            let existing=self.file(&path)?;
            Some(existing.metadata()?.permissions())
        };
        let temp=format!(".endlessvibe-{}.tmp",crate::util::random_secret()?);
        let mut options=OpenOptions::new();
        options.write(true).create_new(true);
        let mut file=dir.open_with(&temp,&options)?.into_std();
        let result=(|| -> Result<()> {
            file.write_all(bytes)?;
            if let Some(permissions)=previous {file.set_permissions(permissions)?;}
            file.sync_all()?;
            // A hard-link/create-only publish is atomic and refuses overwrite.
            // It is immediately reduced to one link by removing the temporary name.
            if create_only {dir.hard_link(&temp,&dir,name)?;}
            else {dir.rename(&temp,&dir,name)?;}
            Ok(())
        })();
        let _=dir.remove_file(&temp);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn portable_root_enforces_boundaries_and_single_links() {
        let d=tempfile::tempdir().unwrap();
        let outside=tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"),b"private").unwrap();
        let root=Root::open(d.path()).unwrap();
        root.replace("src/a.txt",b"hello",true,true).unwrap();
        assert_eq!(root.read("src/a.txt",20).unwrap(),b"hello");
        assert!(root.replace("src/a.txt",b"new",true,false).is_err());
        root.replace("src/a.txt",b"new",false,false).unwrap();
        assert_eq!(root.read("src/a.txt",20).unwrap(),b"new");
        std::fs::hard_link(d.path().join("src/a.txt"),d.path().join("hard")).unwrap();
        assert!(root.read("hard",40).is_err());
        for bad in ["../secret.txt","NUL","CON.txt",".git/config","x/.env","C:foo","trailing."] {
            assert!(relative(bad,false).is_err(),"{bad}");
        }
        #[cfg(unix)] {
            std::os::unix::fs::symlink(outside.path(), d.path().join("escape")).unwrap();
            assert!(root.read("escape/secret.txt",200).is_err());
            assert!(root.replace("escape/secret.txt",b"bad",false,false).is_err());
        }
    }
}
