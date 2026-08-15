use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use walkdir::WalkDir;

use crate::note::Note;

#[derive(Debug, Clone)]
pub struct VaultFile {
    pub path: String,
    pub mtime: i64,
    pub size: i64,
}

#[derive(Debug, Clone)]
pub struct NoteMeta {
    pub path: String,
    pub title: String,
}

#[derive(Debug)]
pub struct VaultError(pub String);

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for VaultError {}

impl From<std::io::Error> for VaultError {
    fn from(e: std::io::Error) -> Self {
        VaultError(e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn new(root: PathBuf) -> Self {
        Vault { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn read_note(&self, relative: &str) -> Result<Note, VaultError> {
        let raw = self.read_raw(relative)?;
        Ok(Note::parse(relative, &raw))
    }

    pub fn read_raw(&self, relative: &str) -> Result<String, VaultError> {
        let full = self.resolve(relative)?;
        if !full.exists() {
            return Err(VaultError(format!("note not found: {relative}")));
        }
        Ok(fs::read_to_string(full)?)
    }

    pub fn write_note(&self, relative: &str, content: &str) -> Result<(), VaultError> {
        let full = self.resolve(relative)?;
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(full, content)?;
        Ok(())
    }

    pub fn delete_note(&self, relative: &str) -> Result<(), VaultError> {
        let full = self.resolve(relative)?;
        if !full.exists() {
            return Err(VaultError(format!("note not found: {relative}")));
        }
        let trash = self.root.join(".trash");
        fs::create_dir_all(&trash)?;
        let dest = trash.join(relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(full, dest)?;
        Ok(())
    }

    pub fn list_files(&self) -> Vec<VaultFile> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| !is_ignored_dir(e.path(), &self.root))
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() || !is_markdown(entry.path()) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(rel) = entry.path().strip_prefix(&self.root) else { continue };
            files.push(VaultFile {
                path: rel.to_string_lossy().replace('\\', "/"),
                mtime: meta.modified().map_or(0, seconds_since_epoch),
                size: meta.len() as i64,
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }

    pub fn list_notes(&self) -> Vec<NoteMeta> {
        self.list_files()
            .into_iter()
            .map(|f| NoteMeta {
                title: file_stem(&f.path).to_string(),
                path: f.path,
            })
            .collect()
    }

    fn resolve(&self, relative: &str) -> Result<PathBuf, VaultError> {
        if relative.is_empty() {
            return Err(VaultError("note path is empty".to_string()));
        }
        let path = Path::new(relative);
        let mut first = true;
        for component in path.components() {
            match component {
                Component::Normal(part) => {
                    if first && (part == ".trash" || part.to_string_lossy().starts_with('.')) {
                        return Err(VaultError(format!("path not allowed: {relative}")));
                    }
                }
                _ => return Err(VaultError(format!("path not allowed: {relative}"))),
            }
            first = false;
        }
        if !is_markdown(path) {
            return Err(VaultError("note path must end in .md".to_string()));
        }
        Ok(self.root.join(path))
    }
}

fn is_markdown(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "md")
}

fn is_ignored_dir(path: &Path, root: &Path) -> bool {
    if path == root {
        return false;
    }
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

fn file_stem(path: &str) -> &str {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
}

fn seconds_since_epoch(time: std::time::SystemTime) -> i64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(dir: &tempfile::TempDir) -> Vault {
        Vault::new(dir.path().to_path_buf())
    }

    #[test]
    fn writes_reads_and_lists_notes() {
        let dir = tempfile::tempdir().unwrap();
        let vault = vault(&dir);
        vault.write_note("personal/one.md", "# One\n\nhello").unwrap();
        vault.write_note("two.md", "# Two\n\nworld").unwrap();
        let note = vault.read_note("personal/one.md").unwrap();
        assert_eq!(note.title, "one");
        let notes = vault.list_notes();
        assert_eq!(notes.len(), 2);
        assert_eq!(notes[0].path, "personal/one.md");
    }

    #[test]
    fn deletes_to_trash() {
        let dir = tempfile::tempdir().unwrap();
        let vault = vault(&dir);
        vault.write_note("x.md", "# X").unwrap();
        vault.delete_note("x.md").unwrap();
        assert!(dir.path().join(".trash/x.md").exists());
        assert!(!dir.path().join("x.md").exists());
    }

    #[test]
    fn rejects_unsafe_paths() {
        let dir = tempfile::tempdir().unwrap();
        let vault = vault(&dir);
        for bad in ["../escape.md", "/abs.md", ".obsidian/app.md", ".trash/x.md", "no-ext", ""] {
            assert!(vault.read_note(bad).is_err(), "should reject {bad}");
        }
    }

    #[test]
    fn ignores_hidden_dirs_when_listing() {
        let dir = tempfile::tempdir().unwrap();
        let vault = vault(&dir);
        vault.write_note("keep.md", "# Keep").unwrap();
        fs::create_dir_all(dir.path().join(".obsidian")).unwrap();
        fs::write(dir.path().join(".obsidian/app.md"), "# App").unwrap();
        let notes = vault.list_notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].path, "keep.md");
    }
}
