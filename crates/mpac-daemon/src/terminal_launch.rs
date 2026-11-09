//! Explicit user-requested shell sessions. Never interpolate commands into an
//! agent's arguments, and freeze the shell before loading session ENV values.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalLaunch {
    pub shell: String,
    pub command: Option<String>,
}

pub fn is_terminal(agent: &str) -> bool {
    matches!(agent, "shell" | "command")
}

#[cfg(unix)]
fn login_shell() -> Option<PathBuf> {
    use std::{
        ffi::{CStr, OsStr},
        os::unix::ffi::OsStrExt,
    };
    let mut buffer = vec![0u8; 16 * 1024];
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found = std::ptr::null_mut();
    // getpwuid_r initializes entry on success and keeps its string pointers
    // inside buffer. Copy pw_shell before either local allocation is dropped.
    unsafe {
        if libc::getpwuid_r(
            libc::getuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        ) != 0
            || found.is_null()
        {
            return None;
        }
        let entry = entry.assume_init();
        if entry.pw_shell.is_null() {
            return None;
        }
        Some(PathBuf::from(OsStr::from_bytes(
            CStr::from_ptr(entry.pw_shell).to_bytes(),
        )))
    }
}

pub fn shell() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let usable = |path: &PathBuf| {
            path.is_absolute()
                && path.is_file()
                && path.to_str().is_some()
                && path
                    .metadata()
                    .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
        };
        std::env::var_os("SHELL")
            .map(PathBuf::from)
            .filter(usable)
            .or_else(|| login_shell().filter(usable))
            .or_else(|| Some(PathBuf::from("/bin/sh")).filter(usable))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

pub fn prepare(agent: &str, command: Option<&str>) -> Result<Option<TerminalLaunch>> {
    if !is_terminal(agent) {
        ensure!(
            command.is_none(),
            "Команда доступна только для запуска произвольной команды"
        );
        return Ok(None);
    }
    if agent == "shell" {
        ensure!(
            command.is_none(),
            "Обычный терминал не принимает команду запуска"
        );
    } else {
        let value = command.context("Введите команду запуска")?;
        ensure!(
            !value.trim().is_empty() && value.len() <= 16 * 1024 && !value.contains('\0'),
            "Команда должна содержать от 1 до 16384 байт без NUL"
        );
    }
    Ok(Some(TerminalLaunch {
        shell: shell()
            .context("Оболочка терминала недоступна")?
            .to_string_lossy()
            .into_owned(),
        command: command.map(str::to_owned),
    }))
}

pub fn build(launch: &TerminalLaunch) -> portable_pty::CommandBuilder {
    let mut command = portable_pty::CommandBuilder::new(&launch.shell);
    if let Some(script) = &launch.command {
        // A one-shot command must not implicitly run login startup files that
        // could change its selected working directory or environment.
        command.args(["-c", script]);
    } else {
        command.args(["-l", "-i"]);
    }
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn scripts_are_explicit_and_preserve_quotes_pipes_and_newlines() {
        for agent in ["claude", "shell", "codex", "demo"] {
            assert!(prepare(agent, Some("echo unexpected")).is_err());
        }
        for bad in [None, Some(" \n\t"), Some("echo\0bad")] {
            assert!(prepare("command", bad).is_err());
        }
        let script = "printf '%s\\n' 'two words' | cat\nprintf done\n";
        let launch = prepare("command", Some(script)).unwrap().unwrap();
        assert_eq!(launch.command.as_deref(), Some(script));
        assert!(PathBuf::from(launch.shell).is_absolute());
        assert!(prepare("shell", None).unwrap().unwrap().command.is_none());
        assert!(prepare("claude", None).unwrap().is_none());
    }
}
