use serde::{Deserialize, Serialize};

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn session_title(title: &str) -> anyhow::Result<String> {
    let title = title.trim();
    anyhow::ensure!(
        !title.is_empty()
            && title.len() <= 120
            && !title
                .chars()
                .any(|c| c.is_control()
                    || matches!(c,'\u{2028}'..='\u{202e}'|'\u{2066}'..='\u{2069}')),
        "Имя сессии должно содержать от 1 до 120 байт без управляющих символов"
    );
    Ok(title.into())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub roots: Vec<mpac_workspace::Root>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTitleSource {
    #[default]
    Manual,
    PendingFirstAnswer,
    Harness,
}

pub fn initial_session_title(
    agent: &str,
    requested: Option<&str>,
) -> anyhow::Result<(String, SessionTitleSource, bool)> {
    if let Some(title) = requested.filter(|s| !s.trim().is_empty()) {
        Ok((session_title(title)?, SessionTitleSource::Manual, false))
    } else {
        Ok((
            session_title(agent)?,
            SessionTitleSource::PendingFirstAnswer,
            true,
        ))
    }
}

#[cfg(test)]
mod title_tests {
    use super::*;
    #[test]
    fn only_omitted_or_empty_launch_title_is_auto_eligible() {
        for requested in [None, Some(""), Some(" \t ")] {
            assert_eq!(
                initial_session_title("codex", requested).unwrap(),
                ("codex".into(), SessionTitleSource::PendingFirstAnswer, true)
            );
        }
        assert_eq!(
            initial_session_title("codex", Some(" My task ")).unwrap(),
            ("My task".into(), SessionTitleSource::Manual, false)
        );
        assert!(initial_session_title("codex", Some("bad\x1btitle")).is_err());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub project_id: String,
    pub root_id: String,
    pub agent: String,
    pub title: String,
    pub status: String,
    pub started_at: u64,
    pub last_activity: u64,
    pub exit_code: Option<u32>,
    pub error: Option<String>,
    pub seq: u64,
    pub mode: String,
    #[serde(default)]
    pub access: crate::access::AccessSettings,
    #[serde(default)]
    pub additional_roots: Vec<mpac_workspace::Root>,
    #[serde(default)]
    pub permission_args: Vec<String>,
    #[serde(default)]
    pub cli_title: Option<String>,
    #[serde(default)]
    pub title_source: SessionTitleSource,
    #[serde(default)]
    pub auto_title_eligible: bool,
    #[serde(default)]
    pub activity: crate::activity::ActivityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<crate::environment::EnvironmentMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_launch: Option<crate::terminal_launch::TerminalLaunch>,
}

impl Session {
    pub fn active(&self) -> bool {
        matches!(self.status.as_str(), "running" | "needs_attention")
    }
}

#[derive(Clone, Serialize)]
pub struct Agent {
    pub id: &'static str,
    pub label: &'static str,
    pub available: bool,
    pub billing: &'static str,
    pub access_modes: Vec<crate::access::ModeDescriptor>,
    pub supports_extra_roots: bool,
    pub supports_network_control: bool,
}

pub fn executable(name: &str) -> Option<std::path::PathBuf> {
    find_executable(
        name,
        &std::env::var_os("PATH")?,
        std::env::current_dir().ok().as_deref(),
    )
}

fn find_executable(
    name: &str,
    search: &std::ffi::OsStr,
    cwd: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    std::env::split_paths(search).find_map(|directory| {
        let selected = directory.join(name);
        // Resolve PATH's relative and empty entries against the daemon cwd now,
        // before the child's project cwd and explicit ENV overlay are applied.
        let path = if selected.is_absolute() {
            selected
        } else {
            cwd?.join(selected)
        };
        if !path.is_file() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if path.metadata().ok()?.permissions().mode() & 0o111 == 0 {
                return None;
            }
        }
        Some(path)
    })
}

#[cfg(all(test, unix))]
mod executable_tests {
    use super::*;
    #[test]
    fn relative_and_empty_path_entries_resolve_before_child_cwd_or_env_changes() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("mpac-executable-{}", id()));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let root = root.canonicalize().unwrap();
        for name in ["claude", "bin/claude"] {
            let file = root.join(name);
            std::fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let relative = find_executable("claude", std::ffi::OsStr::new("bin"), Some(&root)).unwrap();
        assert_eq!(relative, root.join("bin/claude"));
        assert!(relative.is_absolute());
        let empty = find_executable("claude", std::ffi::OsStr::new(""), Some(&root)).unwrap();
        assert_eq!(empty, root.join("claude"));
        std::fs::remove_dir_all(root).unwrap();
    }
}

pub fn agents() -> Vec<Agent> {
    vec![
        Agent {
            id: "demo",
            label: "Демо-агент",
            available: cfg!(unix),
            billing: "Локальный сценарий, без модели и расходов",
            access_modes: crate::access::modes("demo"),
            supports_extra_roots: false,
            supports_network_control: false,
        },
        Agent {
            id: "claude",
            label: "Claude Code",
            available: cfg!(unix) && executable("claude").is_some(),
            billing: "Авторизация оригинального CLI. API-переменные удалены из окружения запуска; account extra usage не проверен",
            access_modes: crate::access::modes("claude"),
            supports_extra_roots: true,
            supports_network_control: false,
        },
        Agent {
            id: "codex",
            label: "Codex CLI",
            available: cfg!(unix) && executable("codex").is_some(),
            billing: "Авторизация оригинального CLI. API-переменные удалены из окружения запуска; account extra usage не проверен",
            access_modes: crate::access::modes("codex"),
            supports_extra_roots: true,
            supports_network_control: true,
        },
        Agent {
            id: "hermes",
            label: "Hermes",
            available: cfg!(unix) && executable("hermes").is_some(),
            billing: "Текущий профиль Hermes / OpenRouter, расходы из кредитного баланса",
            access_modes: crate::access::modes("hermes"),
            supports_extra_roots: false,
            supports_network_control: false,
        },
        Agent {
            id: "zai",
            label: "Z.ai через Claude Code",
            available: false,
            billing: "Изолированный Coding Plan профиль ещё не настроен",
            access_modes: vec![],
            supports_extra_roots: false,
            supports_network_control: false,
        },
    ]
}

/// Kept separate from the agent catalog so older clients never offer a
/// custom command launcher without a command input field.
pub fn terminal_launchers() -> Vec<Agent> {
    vec![
        Agent {
            id: "shell",
            label: "Терминал без агента",
            available: crate::terminal_launch::shell().is_some(),
            billing: "Интерактивная оболочка в рабочей папке проекта",
            access_modes: crate::access::modes("shell"),
            supports_extra_roots: false,
            supports_network_control: false,
        },
        Agent {
            id: "command",
            label: "Произвольная команда",
            available: crate::terminal_launch::shell().is_some(),
            billing: "Команда выполняется вашей оболочкой в рабочей папке проекта",
            access_modes: crate::access::modes("command"),
            supports_extra_roots: false,
            supports_network_control: false,
        },
    ]
}
