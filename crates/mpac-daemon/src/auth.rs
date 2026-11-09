//! One local credential per data directory, retained across daemon updates.
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, ensure};

fn valid(token: &str) -> bool {
    token.len() == 72
        && token.is_ascii()
        && [0, 36].into_iter().all(|start| {
            uuid::Uuid::parse_str(&token[start..start + 36])
                .is_ok_and(|id| !id.is_nil() && id.to_string() == token[start..start + 36])
        })
}

/// Caller holds the exclusive data-directory daemon lock. Never rotate an
/// existing unreadable/invalid credential silently, or follow a token symlink.
pub fn load_or_create(data: &Path) -> Result<String> {
    let path = data.join("auth-token");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    match options.open(&path) {
        Ok(mut file) => {
            let metadata = file.metadata()?;
            ensure!(
                metadata.is_file() && metadata.len() == 72,
                "Некорректный локальный auth-token"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            let mut token = String::new();
            Read::by_ref(&mut file)
                .take(73)
                .read_to_string(&mut token)?;
            ensure!(valid(&token), "Некорректный локальный auth-token");
            Ok(token)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let token = format!("{}{}", crate::model::id(), crate::model::id());
            let temporary = data.join(format!(".auth-token-{}.tmp", crate::model::id()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .context("Не удалось сохранить локальное подключение")?;
            let publish = (|| -> Result<()> {
                file.write_all(token.as_bytes())?;
                file.sync_all()?;
                // Publish complete bytes atomically without replacing a token
                // that unexpectedly appeared. The daemon lock serializes us.
                fs::hard_link(&temporary, &path)?;
                Ok(())
            })();
            drop(file);
            let cleanup = fs::remove_file(&temporary);
            publish.context("Не удалось сохранить локальное подключение")?;
            cleanup?;
            Ok(token)
        }
        Err(error) => Err(error).context("Не удалось прочитать локальный auth-token"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("mpac-auth-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn credential_survives_restart_and_is_unique_per_data_directory() {
        let first = Fixture::new();
        let second = Fixture::new();
        let token = load_or_create(&first.0).unwrap();
        assert!(valid(&token));
        assert_eq!(load_or_create(&first.0).unwrap(), token);
        assert_ne!(load_or_create(&second.0).unwrap(), token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(first.0.join("auth-token"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn corrupted_credential_fails_without_rotation() {
        let fixture = Fixture::new();
        let path = fixture.0.join("auth-token");
        for value in ["invalid".to_string(), "x".repeat(72), "é".repeat(36)] {
            fs::write(&path, &value).unwrap();
            assert!(load_or_create(&fixture.0).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), value);
        }
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_and_non_regular_credentials() {
        let fixture = Fixture::new();
        let target = Fixture::new();
        let token = load_or_create(&target.0).unwrap();
        let path = fixture.0.join("auth-token");
        std::os::unix::fs::symlink(target.0.join("auth-token"), &path).unwrap();
        assert!(load_or_create(&fixture.0).is_err());
        assert_eq!(load_or_create(&target.0).unwrap(), token);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(load_or_create(&fixture.0).is_err());
    }
}
