//! A selected directory is a capability, never a global host path supplied by JavaScript.
use base64::{Engine, engine::general_purpose::STANDARD};
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read, Write},
    path::Path,
};

pub const FILE_LIMIT: usize = 16 * 1024 * 1024;
pub const TOTAL_LIMIT: usize = 64 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct File {
    pub path: String,
    pub data: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub version: u8,
    pub directories: Vec<String>,
    pub files: Vec<File>,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
pub fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\\', ':', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.starts_with('.'))
}
fn decode(workspace: &Workspace) -> io::Result<BTreeMap<String, Vec<u8>>> {
    if workspace.version != 1 || workspace.files.len() > 100 || workspace.directories.len() > 256 {
        return Err(invalid("Workspace exceeds limits"));
    }
    let mut result = BTreeMap::new();
    let mut total = 0;
    for directory in &workspace.directories {
        if !valid_path(directory) {
            return Err(invalid("Invalid folder path (hidden paths are excluded)"));
        }
    }
    for file in &workspace.files {
        if !valid_path(&file.path) || file.data.len() > FILE_LIMIT.div_ceil(3) * 4 {
            return Err(invalid("Invalid file path or size"));
        }
        let bytes = STANDARD
            .decode(&file.data)
            .map_err(|_| invalid("Invalid file encoding"))?;
        total += bytes.len();
        if bytes.len() > FILE_LIMIT
            || total > TOTAL_LIMIT
            || result.insert(file.path.clone(), bytes).is_some()
        {
            return Err(invalid("Duplicate file or workspace exceeds limits"));
        }
    }
    for path in result.keys() {
        if workspace.directories.contains(path)
            || result
                .keys()
                .any(|other| other.starts_with(&format!("{path}/")))
        {
            return Err(invalid("File and directory conflict"));
        }
    }
    Ok(result)
}
fn read_file(dir: &Dir, path: &str) -> io::Result<Option<Vec<u8>>> {
    let metadata = match dir.symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > FILE_LIMIT as u64
    {
        return Err(invalid("Unsupported file, link or oversized file"));
    }
    let mut bytes = Vec::new();
    dir.open(path)?
        .take((FILE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > FILE_LIMIT {
        return Err(invalid("File exceeds limit"));
    }
    Ok(Some(bytes))
}
/// Links, hidden files and special files are never imported. Cap-std also resolves
/// every open relative to the grant and rejects symlink traversal outside it.
pub struct Grant {
    dir: Dir,
    baseline: BTreeMap<String, Vec<u8>>,
}
impl Grant {
    pub fn open(path: &Path) -> io::Result<(Self, Workspace)> {
        let dir = Dir::open_ambient_dir(path, cap_std::ambient_authority())?;
        let mut workspace = Workspace {
            version: 1,
            directories: vec![],
            files: vec![],
        };
        let mut queue = vec![String::new()];
        let mut total = 0;
        while let Some(parent) = queue.pop() {
            for entry in dir.read_dir(if parent.is_empty() { "." } else { &parent })? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = if parent.is_empty() {
                    name
                } else {
                    format!("{parent}/{name}")
                };
                if !valid_path(&path) {
                    continue;
                }
                let kind = entry.file_type()?;
                if kind.is_symlink() {
                    continue;
                }
                if kind.is_dir() {
                    workspace.directories.push(path.clone());
                    queue.push(path);
                    if workspace.directories.len() > 256 {
                        return Err(invalid("Directory exceeds 256 folders"));
                    }
                } else if kind.is_file() {
                    let data = read_file(&dir, &path)?
                        .ok_or_else(|| invalid("File disappeared during import"))?;
                    total += data.len();
                    if total > TOTAL_LIMIT || workspace.files.len() >= 100 {
                        return Err(invalid("Directory exceeds 100 files / 64 MiB"));
                    }
                    workspace.files.push(File {
                        path,
                        data: STANDARD.encode(data),
                    });
                }
            }
        }
        let baseline = decode(&workspace)?;
        Ok((Self { dir, baseline }, workspace))
    }
    /// Update changed files only. Outside edits cause a conflict, never overwrite.
    /// Files removed from the virtual snapshot are retained on disk deliberately.
    pub fn sync(&mut self, workspace: &Workspace) -> io::Result<()> {
        let incoming = decode(workspace)?;
        let changed: Vec<_> = incoming
            .iter()
            .filter(|(path, bytes)| self.baseline.get(*path) != Some(*bytes))
            .collect();
        for (path, _) in &changed {
            if read_file(&self.dir, path)?.as_ref() != self.baseline.get(*path) {
                return Err(invalid(&format!(
                    "External edit conflict: {path}. Reopen the folder to reload."
                )));
            }
        }
        let mut directories: BTreeSet<String> = workspace.directories.iter().cloned().collect();
        for path in incoming.keys() {
            let mut parent = Path::new(path).parent();
            while let Some(value) = parent {
                if !value.as_os_str().is_empty() {
                    directories.insert(value.to_string_lossy().into_owned());
                }
                parent = value.parent();
            }
        }
        if directories.len() > 256 {
            return Err(invalid("Workspace exceeds 256 folders including parents"));
        }
        for directory in directories {
            self.dir.create_dir_all(directory)?;
        }
        for (path, bytes) in changed {
            let temporary = format!(".lethetic-{}.tmp", uuid::Uuid::new_v4());
            let result = (|| {
                let mut file = self
                    .dir
                    .open_with(&temporary, OpenOptions::new().write(true).create_new(true))?;
                file.write_all(bytes)?;
                file.sync_all()?;
                self.dir.rename(&temporary, &self.dir, path)?;
                Ok::<_, io::Error>(())
            })();
            if result.is_err() {
                let _ = self.dir.remove_file(&temporary);
            }
            result?;
            self.baseline.insert(path.clone(), bytes.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn file(path: &str, bytes: &[u8]) -> File {
        File {
            path: path.into(),
            data: STANDARD.encode(bytes),
        }
    }
    #[test]
    fn round_trip_conflicts_and_no_deletion() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("one.txt"), b"before").unwrap();
        let (mut grant, mut workspace) = Grant::open(root.path()).unwrap();
        workspace.files.push(file("nested/two.txt", b"after"));
        grant.sync(&workspace).unwrap();
        assert_eq!(
            std::fs::read(root.path().join("nested/two.txt")).unwrap(),
            b"after"
        );
        std::fs::write(root.path().join("one.txt"), b"external").unwrap();
        workspace.files[0] = file("one.txt", b"overwrite");
        assert!(grant.sync(&workspace).is_err());
        workspace.files.clear();
        grant.sync(&workspace).unwrap();
        assert!(root.path().join("one.txt").exists());
    }
    #[test]
    fn rejects_traversal_hidden_and_duplicates() {
        for path in [
            "../secret",
            "/etc/passwd",
            "C:/x",
            "a\\b",
            "a/./b",
            ".env",
            "a/.git/config",
        ] {
            assert!(!valid_path(path), "{path}");
        }
        let workspace = Workspace {
            version: 1,
            directories: vec![],
            files: vec![file("a", b""), file("a", b"")],
        };
        assert!(decode(&workspace).is_err());
    }
    #[test]
    fn counts_implicit_parent_folders_before_creating_anything() {
        let root = tempfile::tempdir().unwrap();
        let (mut grant, mut workspace) = Grant::open(root.path()).unwrap();
        workspace
            .files
            .push(file(&format!("{}file", "a/".repeat(257)), b"no"));
        assert!(grant.sync(&workspace).is_err());
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_even_after_grant() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let (mut grant, mut workspace) = Grant::open(root.path()).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        workspace.files.push(file("escape/no.txt", b"no"));
        assert!(grant.sync(&workspace).is_err());
        assert!(!outside.path().join("no.txt").exists());
    }
}
