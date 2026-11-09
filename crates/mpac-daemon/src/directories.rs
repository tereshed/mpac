//! Authenticated host directory chooser. This deliberately browses directories
//! outside registered projects, but never reads file contents or mutates them.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

const MAX_ENTRIES: usize = 512;
const MAX_SCANNED: usize = 8192;

#[derive(Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
}

#[derive(Serialize)]
pub struct DirectoryListing {
    pub path: String,
    pub home: String,
    pub parent: Option<String>,
    pub entries: Vec<DirectoryEntry>,
    pub truncated: bool,
}

pub fn list_directories(requested: Option<&str>, show_hidden: bool) -> Result<DirectoryListing> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .context("Домашняя папка не определена")?;
    list_from(&home, requested, show_hidden)
}

fn list_from(home: &Path, requested: Option<&str>, show_hidden: bool) -> Result<DirectoryListing> {
    let home = home.canonicalize().context("Домашняя папка недоступна")?;
    let requested = requested.filter(|s| !s.is_empty());
    let path = requested.map_or(home.as_path(), Path::new);
    ensure!(path.is_absolute(), "Для навигации нужен абсолютный путь");
    ensure!(path.as_os_str().len() <= 4096, "Путь слишком длинный");
    let path = path
        .canonicalize()
        .context("Папка не найдена или недоступна")?;
    ensure!(path.is_dir(), "Выберите папку, а не файл");
    let entries = std::fs::read_dir(&path).context("Нет доступа к содержимому папки")?;
    let mut folders = Vec::new();
    let mut truncated = false;
    for (scanned, entry) in entries.enumerate() {
        if scanned >= MAX_SCANNED {
            truncated = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() && !(kind.is_symlink() && entry.path().is_dir()) {
            continue;
        }
        if folders.len() == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let Some(child_path) = entry.path().to_str().map(str::to_owned) else {
            continue;
        };
        folders.push(DirectoryEntry {
            name,
            path: child_path,
        });
    }
    folders.sort_by_cached_key(|entry| entry.name.to_lowercase());
    Ok(DirectoryListing {
        path: path.to_str().context("Путь не поддерживает UTF-8")?.into(),
        home: home.to_str().context("Путь не поддерживает UTF-8")?.into(),
        parent: path.parent().and_then(Path::to_str).map(str::to_owned),
        entries: folders,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("mpac-folders-{}", crate::model::id()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn defaults_home_and_lists_only_directories_with_hidden_toggle() {
        let root = Temp::new();
        for name in ["Zebra", "проект с пробелами", ".hidden"] {
            std::fs::create_dir(root.0.join(name)).unwrap();
        }
        std::fs::write(root.0.join("private.txt"), "not part of directory picker").unwrap();
        let listing = list_from(&root.0, None, false).unwrap();
        assert_eq!(listing.path, root.0.to_str().unwrap());
        assert_eq!(listing.home, listing.path);
        assert_eq!(listing.entries.len(), 2);
        assert!(
            listing
                .entries
                .iter()
                .any(|e| e.name == "проект с пробелами")
        );
        assert!(
            listing
                .entries
                .iter()
                .all(|e| Path::new(&e.path).is_absolute())
        );
        assert_eq!(list_from(&root.0, None, true).unwrap().entries.len(), 3);
    }

    #[test]
    fn navigation_can_select_empty_directory_and_returns_parent() {
        let root = Temp::new();
        let child = root.0.join("empty");
        std::fs::create_dir(&child).unwrap();
        let listing = list_from(&root.0, child.to_str(), false).unwrap();
        assert!(listing.entries.is_empty());
        assert_eq!(listing.parent.as_deref(), root.0.to_str());
        // The chooser is allowed outside its home; project file APIs remain
        // separately confined to explicitly selected roots.
        assert!(list_from(&child, root.0.to_str(), false).is_ok());
    }

    #[test]
    fn rejects_relative_paths_files_and_missing_directories() {
        let root = Temp::new();
        std::fs::write(root.0.join("file.md"), "text").unwrap();
        assert!(list_from(&root.0, Some("relative"), false).is_err());
        assert!(list_from(&root.0, root.0.join("file.md").to_str(), false).is_err());
        assert!(list_from(&root.0, root.0.join("missing").to_str(), false).is_err());
    }

    #[test]
    fn large_directory_is_explicitly_truncated() {
        let root = Temp::new();
        for n in 0..=MAX_ENTRIES {
            std::fs::create_dir(root.0.join(n.to_string())).unwrap();
        }
        let listing = list_from(&root.0, None, false).unwrap();
        assert_eq!(listing.entries.len(), MAX_ENTRIES);
        assert!(listing.truncated);
    }
}
