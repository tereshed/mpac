//! Copies only explicitly uploaded bytes. No process input, Enter, ignore-file
//! edits or existing-file overwrite is performed. Session lifetime quotas are
//! enforced separately by the SQLite transaction before this file is committed.
use std::path::Path;

use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_SESSION_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_SESSION_FILES: u64 = 50;
pub const MAX_JSON_BYTES: usize = 14 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub path: String,
    pub size: u64,
    pub mime_type: String,
}

pub fn decode_upload(name: &str, encoded: &str) -> Result<(String, Vec<u8>)> {
    ensure!(
        !name.contains(['/', '\\']),
        "Имя вложения не должно содержать путь"
    );
    let name: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_end_matches([' ', '.']).to_owned();
    ensure!(
        !name.is_empty() && name != "." && name != ".." && name.len() <= 240,
        "Нужно имя файла длиной до 240 байт"
    );
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"));
    let name = if reserved { format!("_{name}") } else { name };
    ensure!(
        encoded.len() <= MAX_FILE_BYTES.div_ceil(3) * 4,
        "Файл превышает лимит 10 MiB"
    );
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("Не удалось декодировать вложение"))?;
    ensure!(bytes.len() <= MAX_FILE_BYTES, "Файл превышает лимит 10 MiB");
    Ok((name, bytes))
}

fn mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"%PDF-") {
        "application/pdf"
    } else if !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok() {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use anyhow::{Context, bail};
    use cap_std::fs::{Dir, DirBuilder, DirBuilderExt, MetadataExt, OpenOptions, OpenOptionsExt};
    use std::{
        ffi::CString,
        io::Write,
        os::fd::{AsRawFd, FromRawFd},
    };

    struct CreatedDirectory {
        parent: Dir,
        name: String,
        identity: (u64, u64),
        created: bool,
    }
    struct CreatedFile {
        parent: Dir,
        name: String,
        identity: (u64, u64),
    }

    pub struct PendingAttachment {
        pub attachment: Attachment,
        created_dirs: Vec<CreatedDirectory>,
        file: Option<CreatedFile>,
        committed: bool,
    }

    fn identity(metadata: &cap_std::fs::Metadata) -> (u64, u64) {
        (metadata.dev(), metadata.ino())
    }

    fn open_child(parent: &Dir, name: &str) -> Result<Dir> {
        let name = CString::new(name).context("Недопустимое имя папки вложений")?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            bail!("Папка вложений недоступна или заменена ссылкой");
        }
        Ok(Dir::from_std_file(unsafe {
            std::fs::File::from_raw_fd(fd)
        }))
    }

    fn directory(
        parent: &Dir,
        name: &str,
        fresh: bool,
        created: &mut Vec<CreatedDirectory>,
    ) -> Result<Dir> {
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        let made = match parent.create_dir_with(name, &builder) {
            Ok(()) => true,
            Err(e) if !fresh && e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(_) => bail!("Не удалось создать папку вложения"),
        };
        let dir = open_child(parent, name)?;
        created.push(CreatedDirectory {
            parent: parent.try_clone()?,
            name: name.into(),
            identity: identity(&dir.dir_metadata()?),
            created: made,
        });
        Ok(dir)
    }

    impl PendingAttachment {
        pub fn verify_path(&self) -> Result<()> {
            for directory in &self.created_dirs {
                ensure!(
                    directory
                        .parent
                        .symlink_metadata(&directory.name)
                        .is_ok_and(|m| m.is_dir() && identity(&m) == directory.identity),
                    "Путь вложения изменился во время загрузки"
                );
            }
            let file = self.file.as_ref().context("Файл вложения недоступен")?;
            ensure!(
                file.parent
                    .symlink_metadata(&file.name)
                    .is_ok_and(|m| m.is_file() && identity(&m) == file.identity),
                "Файл вложения изменился во время загрузки"
            );
            Ok(())
        }
        pub fn commit(mut self) -> Attachment {
            self.committed = true;
            self.attachment.clone()
        }
    }

    impl Drop for PendingAttachment {
        fn drop(&mut self) {
            if self.committed {
                return;
            }
            if let Some(file) = &self.file
                && file
                    .parent
                    .symlink_metadata(&file.name)
                    .is_ok_and(|m| m.is_file() && identity(&m) == file.identity)
            {
                let _ = file.parent.remove_file(&file.name);
            }
            // Never recursively remove a directory: concurrent files belong to
            // their creator. Only our still-identical empty directories go away.
            for dir in self.created_dirs.iter().rev() {
                if !dir.created {
                    continue;
                }
                if dir
                    .parent
                    .symlink_metadata(&dir.name)
                    .is_ok_and(|m| m.is_dir() && identity(&m) == dir.identity)
                {
                    let _ = dir.parent.remove_dir(&dir.name);
                }
            }
        }
    }

    pub fn prepare(
        root: &Path,
        session_id: &str,
        root_identity: (u64, u64),
        name: &str,
        bytes: &[u8],
    ) -> Result<PendingAttachment> {
        use std::os::unix::fs::OpenOptionsExt as _;
        ensure!(bytes.len() <= MAX_FILE_BYTES, "Файл превышает лимит 10 MiB");
        ensure!(
            decode_upload(name, "")?.0 == name,
            "Недопустимое имя файла вложения"
        );
        let session_id = uuid::Uuid::parse_str(session_id)
            .context("Недопустимая сессия вложения")?
            .to_string();
        let root_file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(root)
            .context("Папка сессии недоступна или заменена")?;
        let root_dir = Dir::from_std_file(root_file);
        ensure!(
            identity(&root_dir.dir_metadata()?) == root_identity,
            "Папка заменена после запуска CLI"
        );
        let id = uuid::Uuid::new_v4().to_string();
        let path = root
            .join(".mpac-attachments")
            .join(&session_id)
            .join(&id)
            .join(name);
        let attachment = Attachment {
            id: id.clone(),
            name: name.into(),
            path: path
                .to_str()
                .context("Путь вложения должен поддерживать UTF-8")?
                .into(),
            size: bytes.len() as u64,
            mime_type: mime(bytes).into(),
        };
        let mut pending = PendingAttachment {
            attachment,
            created_dirs: vec![],
            file: None,
            committed: false,
        };
        let attachments = directory(
            &root_dir,
            ".mpac-attachments",
            false,
            &mut pending.created_dirs,
        )?;
        let session = directory(&attachments, &session_id, false, &mut pending.created_dirs)?;
        let upload = directory(&session, &id, true, &mut pending.created_dirs)?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .mode(0o600);
        let mut file = upload
            .open_with(name, &options)
            .context("Не удалось создать файл вложения")?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "Вложение должно быть обычным файлом");
        pending.file = Some(CreatedFile {
            parent: upload.try_clone()?,
            name: name.into(),
            identity: identity(&metadata),
        });
        file.write_all(bytes)
            .context("Не удалось записать вложение")?;
        file.sync_all()
            .context("Не удалось сохранить вложение на диске")?;
        Ok(pending)
    }
}

#[cfg(unix)]
pub use unix::{PendingAttachment, prepare};

#[cfg(not(unix))]
pub struct PendingAttachment {
    pub attachment: Attachment,
}
#[cfg(not(unix))]
impl PendingAttachment {
    pub fn verify_path(&self) -> Result<()> {
        anyhow::bail!("Вложения пока доступны только на macOS/Linux")
    }
    pub fn commit(self) -> Attachment {
        self.attachment
    }
}
#[cfg(not(unix))]
pub fn prepare(
    _root: &Path,
    _session: &str,
    _identity: (u64, u64),
    _name: &str,
    _bytes: &[u8],
) -> Result<PendingAttachment> {
    anyhow::bail!("Вложения пока доступны только на macOS/Linux");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_unicode_and_paths_or_empty_names_are_rejected() {
        let (name, data) =
            decode_upload("  план 🦀\u{0001}:draft?.md ", &STANDARD.encode(b"bytes")).unwrap();
        assert_eq!(name, "план 🦀_:draft?.md".replace([':', '?'], "_"));
        assert_eq!(data, b"bytes");
        for name in ["", " . ", "..", "../outside", "/outside", r"..\outside"] {
            assert!(decode_upload(name, "").is_err());
        }
        assert_eq!(decode_upload("CON.txt", "").unwrap().0, "_CON.txt");
        assert!(decode_upload(&"a".repeat(241), "").is_err());
        assert!(decode_upload("okay", "not base64!").is_err());
    }

    #[test]
    fn decoded_file_limit_accepts_boundary_and_rejects_oversize_before_writing() {
        let encoded = STANDARD.encode(vec![b'x'; MAX_FILE_BYTES]);
        assert_eq!(
            decode_upload("large.bin", &encoded).unwrap().1.len(),
            MAX_FILE_BYTES
        );
        assert!(decode_upload("too-large.bin", &(encoded + "AAAA")).is_err());
        assert!(decode_upload("data.bin", "data:application/octet-stream;base64,AAAA").is_err());
    }

    #[cfg(unix)]
    mod filesystem {
        use super::*;
        use std::{
            os::unix::fs::{MetadataExt, symlink},
            path::PathBuf,
        };
        struct Temp(PathBuf);
        impl Temp {
            fn new() -> Self {
                let p = std::env::temp_dir().join(format!("mpac-upload-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir(&p).unwrap();
                Self(p.canonicalize().unwrap())
            }
            fn identity(&self) -> (u64, u64) {
                let m = self.0.metadata().unwrap();
                (m.dev(), m.ino())
            }
        }
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn only_explicit_prepare_writes_and_commit_preserves_exact_bytes() {
            let root = Temp::new();
            assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 0);
            let session = uuid::Uuid::new_v4().to_string();
            let bytes = b"\x89PNG\r\n\x1a\nselected bytes";
            let pending =
                prepare(&root.0, &session, root.identity(), "скриншот.png", bytes).unwrap();
            pending.verify_path().unwrap();
            let meta = pending.commit();
            assert_eq!(std::fs::read(&meta.path).unwrap(), bytes);
            assert_eq!(meta.mime_type, "image/png");
            assert_eq!(meta.size, bytes.len() as u64);
            assert!(
                Path::new(&meta.path).starts_with(root.0.join(".mpac-attachments").join(session))
            );
            assert!(!root.0.join(".gitignore").exists());
        }

        #[test]
        fn uncommitted_upload_cleans_only_its_owned_file_and_empty_directories() {
            let root = Temp::new();
            let pending = prepare(
                &root.0,
                &uuid::Uuid::new_v4().to_string(),
                root.identity(),
                "note.md",
                b"partial",
            )
            .unwrap();
            let path = pending.attachment.path.clone();
            drop(pending);
            assert!(!Path::new(&path).exists());
            assert!(!root.0.join(".mpac-attachments").exists());
            let pending = prepare(
                &root.0,
                &uuid::Uuid::new_v4().to_string(),
                root.identity(),
                "note.md",
                b"partial",
            )
            .unwrap();
            let path = PathBuf::from(&pending.attachment.path);
            let other = path.parent().unwrap().join("created-by-another-actor");
            std::fs::write(&other, b"keep").unwrap();
            drop(pending);
            assert_eq!(std::fs::read(other).unwrap(), b"keep");
            assert!(!path.exists());
        }

        #[test]
        fn root_storage_and_session_symlinks_are_rejected_without_touching_targets() {
            let root = Temp::new();
            let outside = Temp::new();
            let session = uuid::Uuid::new_v4().to_string();
            symlink(&outside.0, root.0.join(".mpac-attachments")).unwrap();
            assert!(prepare(&root.0, &session, root.identity(), "note", b"no").is_err());
            assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 0);
            std::fs::remove_file(root.0.join(".mpac-attachments")).unwrap();
            std::fs::create_dir(root.0.join(".mpac-attachments")).unwrap();
            symlink(&outside.0, root.0.join(".mpac-attachments").join(&session)).unwrap();
            assert!(prepare(&root.0, &session, root.identity(), "note", b"no").is_err());
            assert_eq!(std::fs::read_dir(&outside.0).unwrap().count(), 0);
            let link = root.0.join("root-link");
            symlink(&outside.0, &link).unwrap();
            assert!(prepare(&link, &session, outside.identity(), "note", b"no").is_err());
            assert!(prepare(&root.0, &session, (0, 0), "note", b"no").is_err());
        }

        #[test]
        fn changed_attachment_path_is_detected_and_cleanup_does_not_delete_replacement() {
            let root = Temp::new();
            let pending = prepare(
                &root.0,
                &uuid::Uuid::new_v4().to_string(),
                root.identity(),
                "note",
                b"original",
            )
            .unwrap();
            let path = PathBuf::from(&pending.attachment.path);
            std::fs::rename(&path, path.with_file_name("original-moved")).unwrap();
            std::fs::write(&path, b"replacement").unwrap();
            assert!(pending.verify_path().is_err());
            drop(pending);
            assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        }
    }
}
