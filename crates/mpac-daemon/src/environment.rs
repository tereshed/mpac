//! Per-launch environment snapshot. Contents never enter session metadata or
//! global environment; the parser is data-only and never invokes a shell.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

const MAX_FILE: usize = 256 * 1024;
const MAX_VALUE: usize = 32 * 1024;
const MAX_KEYS: usize = 256;
const INVALID: &str = "Некорректный ENV-файл: используйте KEY=value или export KEY=value";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnvironmentMetadata {
    pub file: String,
    pub variable_count: usize,
}

pub struct Environment {
    metadata: EnvironmentMetadata,
    values: BTreeMap<String, String>,
}
impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("variable_count", &self.values.len())
            .field("values", &"[redacted]")
            .finish()
    }
}
impl Environment {
    pub fn load(requested: &str, cwd: &Path) -> Result<Self> {
        ensure!(
            !requested.is_empty() && requested.len() <= 4096 && !requested.contains('\0'),
            "Укажите путь к ENV-файлу"
        );
        let requested = Path::new(requested);
        let path = if requested.is_absolute() {
            requested.to_owned()
        } else {
            cwd.join(requested)
        };
        let parent = path
            .parent()
            .context("Укажите путь к ENV-файлу")?
            .canonicalize()
            .context("Папка ENV-файла недоступна")?;
        let path = parent.join(path.file_name().context("Укажите путь к ENV-файлу")?);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options
            .open(&path)
            .context("ENV-файл не найден или недоступен; символьные ссылки не поддерживаются")?;
        let meta = file.metadata().context("ENV-файл недоступен")?;
        ensure!(
            meta.is_file() && meta.len() <= MAX_FILE as u64,
            "ENV должен быть обычным файлом размером до 256 KiB"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o022 == 0,
                "ENV-файл должен принадлежать текущему пользователю и быть недоступен для записи другим"
            );
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take((MAX_FILE + 1) as u64)
            .read_to_end(&mut bytes)
            .context("Не удалось прочитать ENV-файл")?;
        ensure!(
            bytes.len() <= MAX_FILE,
            "ENV должен быть обычным файлом размером до 256 KiB"
        );
        let text = std::str::from_utf8(&bytes).map_err(|_| anyhow::anyhow!(INVALID))?;
        let values = parse(text)?;
        // portable-pty otherwise logs a bad SHELL value and silently replaces
        // it. Validate the explicit overlay before that library sees secrets.
        #[cfg(unix)]
        if let Some(shell) = values.get("SHELL") {
            let executable =
                std::ffi::CString::new(shell.as_bytes()).map_err(|_| anyhow::anyhow!(INVALID))?;
            ensure!(
                Path::new(shell).is_absolute()
                    && fs::metadata(shell).is_ok_and(|m| m.is_file())
                    // Match portable-pty's access(X_OK), including owner/group
                    // selection and ACLs; an arbitrary execute bit is not enough.
                    && unsafe { libc::access(executable.as_ptr(), libc::X_OK) } == 0,
                "Переменная SHELL в ENV должна указывать на исполняемый файл абсолютным путём"
            );
        }
        ensure!(!values.is_empty(), "ENV-файл не содержит переменных");
        Ok(Self {
            metadata: EnvironmentMetadata {
                file: path
                    .to_str()
                    .context("Путь ENV-файла должен поддерживать UTF-8")?
                    .into(),
                variable_count: values.len(),
            },
            values,
        })
    }
    pub fn metadata(&self) -> EnvironmentMetadata {
        self.metadata.clone()
    }
    pub fn apply(&self, command: &mut portable_pty::CommandBuilder) {
        for (key, value) in &self.values {
            command.env(key, value);
        }
    }
}

fn key_valid(key: &str) -> bool {
    key.len() <= 128
        && key
            .as_bytes()
            .first()
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}
fn append(value: &mut String, part: &str) -> Result<()> {
    ensure!(
        value.len() + part.len() <= MAX_VALUE,
        "Значение ENV превышает 32 KiB"
    );
    value.push_str(part);
    Ok(())
}
fn interpolate(
    chars: &[char],
    position: &mut usize,
    values: &BTreeMap<String, String>,
    output: &mut String,
) -> Result<()> {
    let start = *position; // '$' position
    let mut cursor = start + 1;
    let braced = chars.get(cursor) == Some(&'{');
    if braced {
        cursor += 1;
    }
    let first = cursor;
    while chars
        .get(cursor)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    {
        cursor += 1;
    }
    let key: String = chars[first..cursor].iter().collect();
    if braced {
        ensure!(key_valid(&key) && chars.get(cursor) == Some(&'}'), INVALID);
    }
    if !key_valid(&key) {
        append(output, "$")?;
        *position += 1;
        return Ok(());
    }
    let value = values
        .get(&key)
        .cloned()
        .or_else(|| std::env::var(&key).ok())
        .unwrap_or_default();
    append(output, &value)?;
    *position = cursor + usize::from(braced);
    Ok(())
}
fn parse(text: &str) -> Result<BTreeMap<String, String>> {
    ensure!(!text.contains('\0'), INVALID);
    let chars: Vec<_> = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .chars()
        .collect();
    let mut cursor = 0;
    let mut values = BTreeMap::new();
    while cursor < chars.len() {
        while chars.get(cursor).is_some_and(|c| c.is_ascii_whitespace()) {
            cursor += 1;
        }
        if cursor == chars.len() {
            break;
        }
        if chars[cursor] == '#' {
            while chars.get(cursor).is_some_and(|c| *c != '\n') {
                cursor += 1;
            }
            continue;
        }
        if chars.get(cursor..cursor + 6) == Some(&['e', 'x', 'p', 'o', 'r', 't'])
            && chars
                .get(cursor + 6)
                .is_some_and(|c| matches!(c, ' ' | '\t'))
        {
            cursor += 6;
            while chars.get(cursor).is_some_and(|c| matches!(c, ' ' | '\t')) {
                cursor += 1;
            }
        }
        let start = cursor;
        while chars
            .get(cursor)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
        {
            cursor += 1;
        }
        let key: String = chars[start..cursor].iter().collect();
        ensure!(key_valid(&key), INVALID);
        ensure!(
            !matches!(key.as_str(), "MPAC_EVENT_CHANNEL" | "MPAC_EVENT_PROVIDER"),
            "ENV-файл содержит служебную переменную MPAC_EVENT_CHANNEL или MPAC_EVENT_PROVIDER"
        );
        while chars.get(cursor).is_some_and(|c| matches!(c, ' ' | '\t')) {
            cursor += 1;
        }
        ensure!(chars.get(cursor) == Some(&'='), INVALID);
        cursor += 1;
        while chars.get(cursor).is_some_and(|c| matches!(c, ' ' | '\t')) {
            cursor += 1;
        }
        let quote = chars
            .get(cursor)
            .copied()
            .filter(|c| matches!(c, '\'' | '"'));
        if quote.is_some() {
            cursor += 1;
        }
        let mut value = String::new();
        let mut closed = quote.is_none();
        while cursor < chars.len() {
            let c = chars[cursor];
            if quote == Some(c) {
                cursor += 1;
                closed = true;
                break;
            }
            if quote.is_none() && matches!(c, '\n' | '\r') {
                break;
            }
            if quote.is_none() && c == '#' && (value.is_empty() || value.ends_with([' ', '\t'])) {
                break;
            }
            if c == '\\' && quote != Some('\'') {
                cursor += 1;
                let Some(escaped) = chars.get(cursor).copied() else {
                    bail!(INVALID)
                };
                let actual = if quote == Some('"') {
                    match escaped {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '\\' | '"' | '$' => escaped,
                        other => {
                            append(&mut value, "\\")?;
                            other
                        }
                    }
                } else {
                    escaped
                };
                append(&mut value, &actual.to_string())?;
                cursor += 1;
                continue;
            }
            if c == '$' && quote != Some('\'') {
                interpolate(&chars, &mut cursor, &values, &mut value)?;
                continue;
            }
            append(&mut value, &c.to_string())?;
            cursor += 1;
        }
        ensure!(closed, INVALID);
        if quote.is_some() {
            while chars.get(cursor).is_some_and(|c| matches!(c, ' ' | '\t')) {
                cursor += 1;
            }
            ensure!(
                chars
                    .get(cursor)
                    .is_none_or(|c| matches!(c, '#' | '\r' | '\n')),
                INVALID
            );
        } else {
            value.truncate(value.trim_end_matches([' ', '\t']).len());
        }
        while chars.get(cursor).is_some_and(|c| !matches!(c, '\n' | '\r')) {
            cursor += 1;
        }
        values.insert(key, value);
        ensure!(
            values.len() <= MAX_KEYS,
            "ENV-файл содержит более 256 переменных"
        );
        ensure!(
            values
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
                <= MAX_FILE,
            "Суммарный размер переменных ENV превышает 256 KiB"
        );
    }
    Ok(values)
}

#[derive(Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
}
#[derive(Serialize)]
pub struct FileListing {
    pub path: String,
    pub home: String,
    pub parent: Option<String>,
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}
pub fn list_files(requested: Option<&str>) -> Result<FileListing> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .context("Домашняя папка не определена")?
        .canonicalize()
        .context("Домашняя папка недоступна")?;
    let path = requested
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.clone());
    ensure!(
        path.is_absolute() && path.as_os_str().len() <= 4096,
        "Для навигации нужен абсолютный путь до 4096 байт"
    );
    let path = path
        .canonicalize()
        .context("Папка не найдена или недоступна")?;
    ensure!(path.is_dir(), "Выберите папку, а не файл");
    let mut entries = Vec::new();
    let mut truncated = false;
    for (index, entry) in fs::read_dir(&path)
        .context("Нет доступа к содержимому папки")?
        .enumerate()
    {
        if index >= 8192 || entries.len() == 512 {
            truncated = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() && !kind.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(child) = entry.path().to_str().map(str::to_owned) else {
            continue;
        };
        entries.push(FileEntry {
            name,
            path: child,
            is_dir: kind.is_dir(),
            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
        });
    }
    entries.sort_by_cached_key(|e| (!e.is_dir, e.name.to_lowercase()));
    Ok(FileListing {
        path: path
            .to_str()
            .context("Путь должен поддерживать UTF-8")?
            .into(),
        home: home
            .to_str()
            .context("Путь должен поддерживать UTF-8")?
            .into(),
        parent: path.parent().and_then(Path::to_str).map(str::to_owned),
        entries,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("mpac-env-{}", crate::model::id()));
            fs::create_dir(&p).unwrap();
            Self(p.canonicalize().unwrap())
        }
        fn file(&self, name: &str, text: &str) -> PathBuf {
            let p = self.0.join(name);
            fs::write(&p, text).unwrap();
            p
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn exports_quotes_comments_unicode_and_literal_shell_are_data() {
        let values=parse("export KEY=first\nKEY='second $HOME'\nTOKEN=abc#def #comment\nMULTI=\"a\\nb\n文\"\nCMD=$(touch /tmp/NEVER)\nBACK=`whoami`\nEMPTY=\n").unwrap();
        assert_eq!(values["KEY"], "second $HOME");
        assert_eq!(values["TOKEN"], "abc#def");
        assert_eq!(values["MULTI"], "a\nb\n文");
        assert_eq!(values["CMD"], "$(touch /tmp/NEVER)");
        assert_eq!(values["BACK"], "`whoami`");
        assert_eq!(values["EMPTY"], "");
    }
    #[test]
    fn interpolation_is_bounded_and_uses_previous_assignments_without_global_changes() {
        let name = format!("MPAC_ENV_TEST_{}", crate::model::id().replace('-', "_"));
        assert!(std::env::var(&name).is_err());
        let values = parse(&format!(
            "{name}=private\nCOPY=${{{name}}}/$UNKNOWN_MPAC_ENV_TEST\nLITERAL=\\$HOME\n"
        ))
        .unwrap();
        assert_eq!(values["COPY"], "private/");
        assert_eq!(values["LITERAL"], "$HOME");
        assert!(std::env::var(&name).is_err());
        let explosive = format!("X={}\nY=$X$X\n", "x".repeat(20 * 1024));
        assert!(parse(&explosive).is_err());
    }
    #[test]
    fn bom_crlf_quotes_comments_empty_values_and_interpolation_precedence() {
        let values=parse("\u{feff}# comment\r\nexport HOME=local\r\nA = \"${HOME}/文\" #comment\r\nEMPTY=   # comment\r\nSINGLE=' $HOME '\r\nDOUBLE=\"\\$HOME\"\r\n").unwrap();
        assert_eq!(values["A"], "local/文");
        assert_eq!(values["EMPTY"], "");
        assert_eq!(values["SINGLE"], " $HOME ");
        assert_eq!(values["DOUBLE"], "$HOME");
        for text in ["X=${NAME:-SECRET_BAD}", "X=${}", "X=${NAME=SECRET_BAD}"] {
            let error = parse(text).unwrap_err();
            assert!(!error.to_string().contains("SECRET_BAD"));
        }
    }
    #[test]
    fn invalid_and_reserved_errors_never_include_secret_values() {
        for text in [
            "SECRET_OWN_99 bad=value",
            "TOKEN=\"SECRET_OWN_99",
            "MPAC_EVENT_CHANNEL=SECRET_OWN_99",
        ] {
            let error = format!("{:#}", parse(text).unwrap_err());
            assert!(!error.contains("SECRET_OWN_99"));
        }
    }
    #[test]
    fn file_load_preserves_spaces_redacts_debug_and_metadata() {
        let temp = Temp::new();
        let file = temp.file(" vars.env ", "TOKEN=secret-value-991\n");
        let env = Environment::load(" vars.env ", &temp.0).unwrap();
        assert_eq!(env.metadata().file, file.to_str().unwrap());
        assert_eq!(env.metadata().variable_count, 1);
        assert!(!format!("{env:?}").contains("secret-value"));
        assert!(
            !serde_json::to_string(&env.metadata())
                .unwrap()
                .contains("secret-value")
        );
        assert!(Environment::load("missing", &temp.0).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlink_fifo_unsafe_permissions_and_large_values() {
        use std::os::unix::{
            ffi::OsStrExt,
            fs::{PermissionsExt, symlink},
        };
        let temp = Temp::new();
        let file = temp.file(".env", "X=ok\n");
        symlink(&file, temp.0.join("link.env")).unwrap();
        assert!(Environment::load("link.env", &temp.0).is_err());
        let fifo = temp.0.join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(Environment::load("fifo", &temp.0).is_err());
        fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(Environment::load(".env", &temp.0).is_err());
        temp.file("big.env", &"x".repeat(MAX_FILE + 1));
        assert!(Environment::load("big.env", &temp.0).is_err());
    }
    #[test]
    fn apply_only_changes_the_explicit_child_builder_and_preserves_parent() {
        let temp = Temp::new();
        temp.file(
            "custom.env",
            "ANTHROPIC_AUTH_TOKEN=explicit-token\nANTHROPIC_BASE_URL=https://provider.invalid\n",
        );
        let env = Environment::load("custom.env", &temp.0).unwrap();
        let before = std::env::var_os("ANTHROPIC_AUTH_TOKEN");
        let mut command = portable_pty::CommandBuilder::new("claude");
        command.env_remove("ANTHROPIC_AUTH_TOKEN");
        env.apply(&mut command);
        assert_eq!(
            command.get_env("ANTHROPIC_AUTH_TOKEN"),
            Some(std::ffi::OsStr::new("explicit-token"))
        );
        assert_eq!(std::env::var_os("ANTHROPIC_AUTH_TOKEN"), before);
    }
    #[cfg(unix)]
    #[test]
    fn bad_explicit_shell_is_redacted_before_pty_can_warn_or_replace_it() {
        let temp = Temp::new();
        temp.file("shell.env", "SHELL=/SECRET_INVALID_SHELL_992\n");
        let error = Environment::load("shell.env", &temp.0).unwrap_err();
        assert!(!format!("{error:#}").contains("SECRET_INVALID_SHELL_992"));
        temp.file("good.env", "SHELL=/bin/sh\n");
        assert!(Environment::load("good.env", &temp.0).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn shell_execute_check_matches_owner_access_and_never_exposes_the_value() {
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
        let temp = Temp::new();
        let shell = temp.file("SECRET_SHELL_NO_OWNER_EXEC", "#!/bin/sh\nexit 0\n");
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o001)).unwrap();
        temp.file("mode.env", &format!("SHELL={}\n", shell.display()));
        let name = std::ffi::CString::new(shell.as_os_str().as_bytes()).unwrap();
        let permitted = unsafe { libc::access(name.as_ptr(), libc::X_OK) } == 0;
        if unsafe { libc::geteuid() } != 0 {
            assert!(!permitted);
        }
        let loaded = Environment::load("mode.env", &temp.0);
        assert_eq!(loaded.is_ok(), permitted);
        if let Err(error) = loaded {
            assert!(!format!("{error:#}").contains("SECRET_SHELL_NO_OWNER_EXEC"));
        }
    }
    #[test]
    fn picker_includes_hidden_arbitrary_files_but_never_contents() {
        let temp = Temp::new();
        temp.file(".env.zai", "SECRET_OWN_992");
        temp.file("variables.custom", "TOKEN=value");
        fs::create_dir(temp.0.join("folder")).unwrap();
        let listing = list_files(temp.0.to_str()).unwrap();
        assert!(listing.entries.iter().any(|e| e.name == ".env.zai"));
        assert!(listing.entries.iter().any(|e| e.name == "variables.custom"));
        assert!(listing.entries[0].is_dir);
        assert!(
            !serde_json::to_string(&listing)
                .unwrap()
                .contains("SECRET_OWN_992")
        );
    }
}
