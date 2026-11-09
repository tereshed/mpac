//! Pure translation of explicit launch settings to each installed CLI's flags.
//!
//! These options configure the CLI. They are not an application-wide sandbox
//! for every tool, hook or MCP server. Inherit deliberately preserves the CLI's
//! existing configuration and must not be presented as a strict safe mode.
//! Codex always runs without its shared daemon so PTY ownership remains local.
//! Explicit modes keep named permission profiles and legacy sandbox settings
//! consistent, including CLI paths with different configuration precedence.

use std::{collections::HashSet, path::Path};

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    #[default]
    Inherit,
    ReadOnly,
    WorkspaceWrite,
    Plan,
    Manual,
    AcceptEdits,
    FullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    OnRequest,
    Never,
}

impl ApprovalPolicy {
    fn argument(self) -> &'static str {
        match self {
            Self::OnRequest => "on-request",
            Self::Never => "never",
        }
    }
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccessSettings {
    pub mode: AccessMode,
    pub approval: Option<ApprovalPolicy>,
    pub network_access: Option<bool>,
    pub additional_root_ids: Vec<String>,
    pub acknowledge_full_access: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModeDescriptor {
    pub id: AccessMode,
    pub label: &'static str,
    pub description: &'static str,
    pub unrestricted: bool,
}

fn descriptor(agent: &str, mode: AccessMode) -> ModeDescriptor {
    let (label, description) = match mode {
        AccessMode::Inherit => (
            "Настройки CLI",
            "Сохранить текущие настройки CLI. Они могут разрешать расширенный доступ.",
        ),
        AccessMode::ReadOnly => (
            "Только чтение",
            "Ограничивает запись команд Codex, без запросов расширения прав. Не изолирует все hooks и MCP.",
        ),
        AccessMode::WorkspaceWrite => (
            "Рабочие и временные папки CLI",
            "Запись команд в рабочую, временные и выбранные дополнительные папки. Hooks и MCP имеют собственные права.",
        ),
        AccessMode::Plan => (
            "Планирование",
            "Claude Code: режим plan. Разрешения задаёт сам CLI.",
        ),
        AccessMode::Manual => (
            "Ручные подтверждения",
            "Claude Code: режим manual с подтверждениями в терминале.",
        ),
        AccessMode::AcceptEdits => (
            "Принимать правки",
            "Claude Code: режим acceptEdits, автоматически принимающий редактирование файлов.",
        ),
        AccessMode::FullAccess => match agent {
            "codex" => (
                "Без sandbox",
                "Отключить sandbox команд Codex. Подтверждения задаются отдельно; требуется подтвердить риск.",
            ),
            "claude" => (
                "Без проверок разрешений",
                "Отключить проверки разрешений Claude Code. Ограничения окружения сохраняются; требуется подтвердить риск.",
            ),
            "hermes" => (
                "Без подтверждений команд",
                "Отключить подтверждения опасных команд Hermes. Ограничения окружения сохраняются; требуется подтвердить риск.",
            ),
            _ => ("Полный доступ", "Режим недоступен для этого агента."),
        },
    };
    ModeDescriptor {
        id: mode,
        label,
        description,
        unrestricted: mode == AccessMode::FullAccess,
    }
}

pub fn modes(agent: &str) -> Vec<ModeDescriptor> {
    use AccessMode::*;
    let modes: &[AccessMode] = match agent {
        "codex" => &[Inherit, ReadOnly, WorkspaceWrite, FullAccess],
        "claude" => &[Inherit, Manual, Plan, AcceptEdits, FullAccess],
        "hermes" => &[Inherit, FullAccess],
        "demo" | "shell" | "command" => &[Inherit],
        _ => &[],
    };
    modes
        .iter()
        .copied()
        .map(|mode| descriptor(agent, mode))
        .collect()
}

/// Paths have already been resolved from frozen project-root IDs by the API.
/// They are passed as individual argv values; this module never invokes a shell.
pub fn launch_args(
    agent: &str,
    settings: &AccessSettings,
    additional_paths: &[String],
) -> Result<Vec<String>> {
    launch_args_inner(agent, settings, additional_paths, None)
}

/// The API must allocate a fresh server-side session UUID before calling this
/// function. A unique named workspace profile prevents lower-layer config from
/// deep-merging additional filesystem grants into a shared application profile.
/// The UUID is not a client-selectable permission profile or a global config edit.
pub fn launch_args_for_session(
    agent: &str,
    settings: &AccessSettings,
    additional_paths: &[String],
    session_id: &str,
) -> Result<Vec<String>> {
    let id = uuid::Uuid::parse_str(session_id)
        .map_err(|_| anyhow::anyhow!("Для профиля доступа нужен UUID новой сессии"))?;
    ensure!(!id.is_nil(), "Для профиля доступа нужен UUID новой сессии");
    launch_args_inner(agent, settings, additional_paths, Some(&id.to_string()))
}

fn launch_args_inner(
    agent: &str,
    settings: &AccessSettings,
    additional_paths: &[String],
    session_id: Option<&str>,
) -> Result<Vec<String>> {
    use AccessMode::*;
    ensure!(
        modes(agent).iter().any(|mode| mode.id == settings.mode),
        "Этот режим доступа недоступен для выбранного агента"
    );
    ensure!(
        settings.additional_root_ids.len() <= 12
            && additional_paths.len() == settings.additional_root_ids.len(),
        "Список дополнительных папок не соответствует выбранным папкам проекта"
    );
    let mut ids = HashSet::new();
    for id in &settings.additional_root_ids {
        ensure!(
            !id.trim().is_empty() && !id.contains('\0') && ids.insert(id),
            "Дополнительная папка пуста или указана дважды"
        );
    }
    let mut paths = HashSet::new();
    for path in additional_paths {
        ensure!(
            !path.contains('\0') && Path::new(path).is_absolute() && paths.insert(path),
            "Дополнительные папки должны иметь разные абсолютные пути без NUL"
        );
    }
    ensure!(
        settings.mode == FullAccess || !settings.acknowledge_full_access,
        "Подтверждение полного доступа применимо только к режиму полного доступа"
    );
    ensure!(
        settings.mode != FullAccess || settings.acknowledge_full_access,
        "Для полного доступа нужно явно подтвердить риск"
    );

    if settings.mode == Inherit {
        ensure!(
            settings.approval.is_none()
                && settings.network_access.is_none()
                && additional_paths.is_empty(),
            "Для настроек подтверждений, сети или дополнительных папок выберите явный режим доступа"
        );
        return Ok(if agent == "codex" {
            vec!["--no-daemon".into()]
        } else {
            Vec::new()
        });
    }

    ensure!(
        additional_paths.is_empty()
            || matches!(
                (agent, settings.mode),
                ("codex", WorkspaceWrite) | ("claude", Manual | Plan | AcceptEdits)
            ),
        "В выбранном режиме дополнительные папки не поддерживаются"
    );

    let mut args: Vec<String> = match agent {
        "codex" => {
            ensure!(
                settings.mode == WorkspaceWrite || settings.network_access.is_none(),
                "Настройка сети доступна только в режиме Codex workspace-write"
            );
            let (sandbox, approval) = match settings.mode {
                ReadOnly => {
                    ensure!(
                        settings.approval.is_none()
                            || settings.approval == Some(ApprovalPolicy::Never),
                        "Codex read-only не запрашивает расширение прав; выберите never"
                    );
                    ("read-only", ApprovalPolicy::Never)
                }
                WorkspaceWrite => (
                    "workspace-write",
                    settings.approval.unwrap_or(ApprovalPolicy::OnRequest),
                ),
                FullAccess => (
                    "danger-full-access",
                    settings.approval.unwrap_or(ApprovalPolicy::OnRequest),
                ),
                _ => unreachable!("supported modes were checked above"),
            };
            let mut args = vec![
                "--no-daemon".into(),
                "--sandbox".into(),
                sandbox.into(),
                "--ask-for-approval".into(),
                approval.argument().into(),
            ];
            let profile = match settings.mode {
                ReadOnly => ":read-only".to_owned(),
                FullAccess => ":danger-full-access".to_owned(),
                WorkspaceWrite => format!(
                    "mpac-{}",
                    session_id.ok_or_else(|| anyhow::anyhow!(
                        "Для workspace-write нужен UUID новой сессии"
                    ))?
                ),
                _ => unreachable!("supported modes were checked above"),
            };
            args.extend(["-c".into(), format!("default_permissions=\"{profile}\"")]);
            if settings.mode == WorkspaceWrite {
                let roots: Result<Vec<_>> = additional_paths
                    .iter()
                    .map(|path| Ok(format!("{}=true", serde_json::to_string(path)?)))
                    .collect();
                args.extend([
                    "-c".into(),
                    format!(
                        "permissions.{profile}={{extends=\":workspace\",workspace_roots={{{}}},network={{enabled={}}}}}",
                        roots?.join(","), settings.network_access.unwrap_or(false)
                    ),
                ]);
                args.extend([
                    "-c".into(),
                    "sandbox_workspace_write.writable_roots=[]".into(),
                    "-c".into(),
                    format!(
                        "sandbox_workspace_write.network_access={}",
                        settings.network_access.unwrap_or(false)
                    ),
                ]);
            }
            if approval == ApprovalPolicy::OnRequest {
                args.extend(["-c".into(), "approvals_reviewer=\"user\"".into()]);
            }
            args
        }
        "claude" => {
            ensure!(
                settings.approval.is_none() && settings.network_access.is_none(),
                "Для Claude Code отдельные настройки approval и network не поддерживаются"
            );
            match settings.mode {
                Manual => vec!["--permission-mode".into(), "manual".into()],
                Plan => vec!["--permission-mode".into(), "plan".into()],
                AcceptEdits => vec!["--permission-mode".into(), "acceptEdits".into()],
                FullAccess => vec!["--dangerously-skip-permissions".into()],
                _ => unreachable!("supported modes were checked above"),
            }
        }
        "hermes" => {
            ensure!(
                settings.approval.is_none() && settings.network_access.is_none(),
                "Для Hermes отдельные настройки approval и network не поддерживаются"
            );
            vec!["--yolo".into()]
        }
        _ => bail!("Этот агент не поддерживает выбранный режим доступа"),
    };
    for path in additional_paths {
        args.extend(["--add-dir".into(), path.clone()]);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "123e4567-e89b-42d3-a456-426614174000";

    fn launch_args(
        agent: &str,
        settings: &AccessSettings,
        paths: &[String],
    ) -> Result<Vec<String>> {
        super::launch_args_for_session(agent, settings, paths, SESSION_ID)
    }

    fn settings(mode: AccessMode) -> AccessSettings {
        AccessSettings {
            mode,
            acknowledge_full_access: mode == AccessMode::FullAccess,
            ..Default::default()
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).into()).collect()
    }

    fn extra_path() -> String {
        if cfg!(windows) {
            r"C:\work\extra folder".into()
        } else {
            "/work/extra folder".into()
        }
    }

    #[test]
    fn serde_defaults_are_inherit_and_unknown_fields_modes_and_policies_are_rejected() {
        let defaults: AccessSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults, AccessSettings::default());
        assert_eq!(defaults.mode, AccessMode::Inherit);
        for json in [
            r#"{"mode":"sandbox"}"#,
            r#"{"approval":"auto"}"#,
            r#"{"dangerously_skip_permissions":true}"#,
        ] {
            assert!(serde_json::from_str::<AccessSettings>(json).is_err());
        }
    }

    #[test]
    fn inherit_preserves_policy_and_only_codex_forces_local_process_ownership() {
        for agent in ["codex", "claude", "hermes", "demo"] {
            let expected = if agent == "codex" {
                strings(&["--no-daemon"])
            } else {
                Vec::new()
            };
            assert_eq!(
                launch_args(agent, &AccessSettings::default(), &[]).unwrap(),
                expected
            );
            assert_eq!(modes(agent)[0].id, AccessMode::Inherit);
        }
        assert!(modes("unknown").is_empty());
        assert!(launch_args("unknown", &AccessSettings::default(), &[]).is_err());
    }

    #[test]
    fn codex_argv_defaults_are_explicit_and_do_not_silently_enable_network() {
        assert_eq!(
            launch_args("codex", &settings(AccessMode::ReadOnly), &[]).unwrap(),
            strings(&[
                "--no-daemon",
                "--sandbox",
                "read-only",
                "--ask-for-approval",
                "never",
                "-c",
                "default_permissions=\":read-only\""
            ])
        );
        assert_eq!(
            launch_args("codex", &settings(AccessMode::WorkspaceWrite), &[]).unwrap(),
            strings(&[
                "--no-daemon",
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "-c",
                "default_permissions=\"mpac-123e4567-e89b-42d3-a456-426614174000\"",
                "-c",
                "permissions.mpac-123e4567-e89b-42d3-a456-426614174000={extends=\":workspace\",workspace_roots={},network={enabled=false}}",
                "-c",
                "sandbox_workspace_write.writable_roots=[]",
                "-c",
                "sandbox_workspace_write.network_access=false",
                "-c",
                "approvals_reviewer=\"user\""
            ])
        );
        assert_eq!(
            launch_args("codex", &settings(AccessMode::FullAccess), &[]).unwrap(),
            strings(&[
                "--no-daemon",
                "--sandbox",
                "danger-full-access",
                "--ask-for-approval",
                "on-request",
                "-c",
                "default_permissions=\":danger-full-access\"",
                "-c",
                "approvals_reviewer=\"user\""
            ])
        );
    }

    #[test]
    fn codex_workspace_options_and_additional_path_are_separate_argv_values() {
        let mut setting = settings(AccessMode::WorkspaceWrite);
        setting.approval = Some(ApprovalPolicy::Never);
        setting.network_access = Some(true);
        setting.additional_root_ids = vec!["other-root".into()];
        let path = extra_path();
        let expected_profile = if cfg!(windows) {
            r#"permissions.mpac-123e4567-e89b-42d3-a456-426614174000={extends=":workspace",workspace_roots={"C:\\work\\extra folder"=true},network={enabled=true}}"#
        } else {
            r#"permissions.mpac-123e4567-e89b-42d3-a456-426614174000={extends=":workspace",workspace_roots={"/work/extra folder"=true},network={enabled=true}}"#
        };
        assert_eq!(
            launch_args("codex", &setting, std::slice::from_ref(&path)).unwrap(),
            vec![
                "--no-daemon".into(),
                "--sandbox".into(),
                "workspace-write".into(),
                "--ask-for-approval".into(),
                "never".into(),
                "-c".into(),
                "default_permissions=\"mpac-123e4567-e89b-42d3-a456-426614174000\"".into(),
                "-c".into(),
                expected_profile.into(),
                "-c".into(),
                "sandbox_workspace_write.writable_roots=[]".into(),
                "-c".into(),
                "sandbox_workspace_write.network_access=true".into(),
                "--add-dir".into(),
                path
            ]
        );
    }

    #[test]
    fn claude_argv_uses_installed_manual_spelling_and_requires_no_codex_knobs() {
        for (mode, expected) in [
            (AccessMode::Manual, vec!["--permission-mode", "manual"]),
            (AccessMode::Plan, vec!["--permission-mode", "plan"]),
            (
                AccessMode::AcceptEdits,
                vec!["--permission-mode", "acceptEdits"],
            ),
            (
                AccessMode::FullAccess,
                vec!["--dangerously-skip-permissions"],
            ),
        ] {
            assert_eq!(
                launch_args("claude", &settings(mode), &[]).unwrap(),
                strings(&expected)
            );
        }
        for mode in [
            AccessMode::Manual,
            AccessMode::Plan,
            AccessMode::AcceptEdits,
        ] {
            let mut setting = settings(mode);
            setting.additional_root_ids = vec!["other-root".into()];
            let path = extra_path();
            let args = launch_args("claude", &setting, std::slice::from_ref(&path)).unwrap();
            assert_eq!(&args[args.len() - 2..], &["--add-dir".to_owned(), path]);
        }
    }

    #[test]
    fn hermes_explicit_full_access_maps_only_to_yolo() {
        assert_eq!(
            launch_args("hermes", &settings(AccessMode::FullAccess), &[]).unwrap(),
            strings(&["--yolo"])
        );
    }

    #[test]
    fn full_access_is_impossible_without_acknowledgment_and_ack_is_not_accepted_elsewhere() {
        for agent in ["codex", "claude", "hermes"] {
            let mut setting = settings(AccessMode::FullAccess);
            setting.acknowledge_full_access = false;
            assert!(launch_args(agent, &setting, &[]).is_err());
        }
        let setting = AccessSettings {
            acknowledge_full_access: true,
            ..Default::default()
        };
        assert!(launch_args("codex", &setting, &[]).is_err());
        for agent in ["codex", "claude", "hermes"] {
            assert!(
                modes(agent)
                    .iter()
                    .filter(|m| m.unrestricted)
                    .all(|m| m.id == AccessMode::FullAccess)
            );
        }
    }

    #[test]
    fn mismatched_profiles_and_unsupported_knobs_are_rejected_instead_of_ignored() {
        for (agent, mode) in [
            ("codex", AccessMode::Manual),
            ("codex", AccessMode::Plan),
            ("claude", AccessMode::ReadOnly),
            ("claude", AccessMode::WorkspaceWrite),
            ("hermes", AccessMode::WorkspaceWrite),
            ("demo", AccessMode::FullAccess),
        ] {
            assert!(launch_args(agent, &settings(mode), &[]).is_err());
        }
        for agent in ["claude", "hermes"] {
            let mut setting = settings(AccessMode::FullAccess);
            setting.approval = Some(ApprovalPolicy::Never);
            assert!(launch_args(agent, &setting, &[]).is_err());
            setting.approval = None;
            setting.network_access = Some(false);
            assert!(launch_args(agent, &setting, &[]).is_err());
        }
        for mode in [AccessMode::ReadOnly, AccessMode::FullAccess] {
            let mut setting = settings(mode);
            setting.network_access = Some(false);
            assert!(launch_args("codex", &setting, &[]).is_err());
        }
        let mut setting = settings(AccessMode::ReadOnly);
        setting.approval = Some(ApprovalPolicy::OnRequest);
        assert!(launch_args("codex", &setting, &[]).is_err());
    }

    #[test]
    fn inherit_rejects_approval_network_and_additional_paths_for_all_agents() {
        for agent in ["codex", "claude", "hermes", "demo"] {
            let mut setting = AccessSettings {
                approval: Some(ApprovalPolicy::Never),
                ..Default::default()
            };
            assert!(launch_args(agent, &setting, &[]).is_err());
            setting.approval = None;
            setting.network_access = Some(false);
            assert!(launch_args(agent, &setting, &[]).is_err());
            setting.network_access = None;
            setting.additional_root_ids = vec!["extra".into()];
            assert!(launch_args(agent, &setting, &[extra_path()]).is_err());
        }
    }

    #[test]
    fn additional_paths_require_supported_modes_and_matching_unique_ids() {
        for (agent, mode) in [
            ("codex", AccessMode::ReadOnly),
            ("codex", AccessMode::FullAccess),
            ("claude", AccessMode::FullAccess),
            ("hermes", AccessMode::FullAccess),
        ] {
            let mut setting = settings(mode);
            setting.additional_root_ids = vec!["extra".into()];
            assert!(launch_args(agent, &setting, &[extra_path()]).is_err());
        }
        let mut setting = settings(AccessMode::WorkspaceWrite);
        assert!(launch_args("codex", &setting, &[extra_path()]).is_err());
        setting.additional_root_ids = vec!["extra".into()];
        assert!(launch_args("codex", &setting, &[]).is_err());
        for invalid in ["relative/path", "", "/path\0withnul"] {
            assert!(launch_args("codex", &setting, &[invalid.into()]).is_err());
        }
        setting.additional_root_ids = vec!["extra".into(), "extra".into()];
        assert!(launch_args("codex", &setting, &[extra_path(), extra_path()]).is_err());
        setting.additional_root_ids = (0..13).map(|i| format!("root-{i}")).collect();
        let paths: Vec<_> = (0..13).map(|i| format!("/extra/{i}")).collect();
        assert!(launch_args("codex", &setting, &paths).is_err());
    }

    #[test]
    fn workspace_profile_requires_a_fresh_session_uuid_and_is_not_a_common_profile_name() {
        let setting = settings(AccessMode::WorkspaceWrite);
        assert!(super::launch_args("codex", &setting, &[]).is_err());
        for invalid in [
            "",
            "mpac-shared",
            "\"malicious\"",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(super::launch_args_for_session("codex", &setting, &[], invalid).is_err());
        }
        let first = super::launch_args_for_session("codex", &setting, &[], SESSION_ID).unwrap();
        let second = super::launch_args_for_session(
            "codex",
            &setting,
            &[],
            "de305d54-75b4-431b-adb2-eb6b9e546014",
        )
        .unwrap();
        assert_ne!(first, second);
        assert!(
            first
                .iter()
                .any(|arg| arg == "sandbox_workspace_write.writable_roots=[]")
        );
        assert!(first.iter().any(|arg| arg == "approvals_reviewer=\"user\""));
        assert!(
            first
                .iter()
                .any(|arg| arg
                    == "default_permissions=\"mpac-123e4567-e89b-42d3-a456-426614174000\"")
        );
    }
}
