use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Default)]
pub struct Options {
    pub data_dir: Option<PathBuf>,
    pub profile_dir: Option<PathBuf>,
    pub probe: bool,
    pub help: bool,
}

impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut args = args.into_iter();
        let mut result = Self::default();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    result.data_dir = Some(PathBuf::from(
                        args.next().context("Missing --data-dir path")?,
                    ))
                }
                "--profile-dir" => {
                    result.profile_dir = Some(PathBuf::from(
                        args.next().context("Missing --profile-dir path")?,
                    ))
                }
                "--probe" => result.probe = true,
                "--help" => result.help = true,
                value if value.starts_with("-psn_") => {}
                _ => bail!("Unknown desktop option"),
            }
        }
        Ok(result)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub data_dir: PathBuf,
}

pub fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)?;
    }
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "Desktop profile must be a real directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
            "Desktop profile must be private to its owner"
        );
    }
    Ok(())
}

pub fn load(profile: &Path, override_dir: Option<&Path>) -> Result<Settings> {
    private_directory(profile)?;
    if let Some(data_dir) = override_dir {
        return Ok(Settings {
            data_dir: data_dir.to_owned(),
        });
    }
    let path = profile.join("native.json");
    match fs::symlink_metadata(&path) {
        Ok(meta) => ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Desktop settings must be a regular file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings {
                data_dir: profile.join("daemon"),
            });
        }
        Err(error) => return Err(error.into()),
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).context("Cannot read desktop settings")?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file() && meta.len() <= 16384,
        "Invalid desktop settings file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0,
            "Desktop settings must be private to their owner"
        );
    }
    let mut bytes = Vec::new();
    file.take(16385).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16384, "Desktop settings are too large");
    serde_json::from_slice(&bytes).context("Invalid desktop settings")
}

pub fn save(profile: &Path, data_dir: &Path) -> Result<()> {
    private_directory(profile)?;
    let path = profile.join("native.json");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = profile.join(format!(".native-{}-{nonce}.json", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| -> Result<()> {
        file.write_all(&serde_json::to_vec_pretty(&Settings {
            data_dir: data_dir.to_owned(),
        })?)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn options_keep_literal_paths_and_reject_unknown_flags() {
        let args = [
            "--data-dir",
            "/some folder/проект",
            "--profile-dir",
            "/private/native",
            "--probe",
        ]
        .map(str::to_owned);
        let parsed = Options::parse(args).unwrap();
        assert_eq!(
            parsed.data_dir.unwrap(),
            PathBuf::from("/some folder/проект")
        );
        assert!(parsed.probe);
        assert!(Options::parse(["--url=https://example.com".into()]).is_err());
        assert!(Options::parse(["--data-dir".into()]).is_err());
    }
    #[test]
    fn preference_contains_only_data_path_and_rejects_symlinks() {
        let profile = std::env::temp_dir().join(format!(
            "mpac-native-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        save(&profile, Path::new("/private/Мой проект")).unwrap();
        assert_eq!(
            load(&profile, None).unwrap().data_dir,
            PathBuf::from("/private/Мой проект")
        );
        let data = fs::read_to_string(profile.join("native.json")).unwrap();
        assert!(!data.contains("token"));
        #[cfg(unix)]
        {
            fs::rename(profile.join("native.json"), profile.join("other.json")).unwrap();
            std::os::unix::fs::symlink(profile.join("other.json"), profile.join("native.json"))
                .unwrap();
            assert!(load(&profile, None).is_err());
            fs::remove_file(profile.join("other.json")).unwrap();
            assert!(
                load(&profile, None).is_err(),
                "dangling settings links must not select a fresh workspace"
            );
        }
        fs::remove_dir_all(profile).unwrap();
    }
}
