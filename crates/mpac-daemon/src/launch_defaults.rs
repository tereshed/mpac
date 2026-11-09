//! Persisted launch presets contain access choices, never CLI arguments or credentials.
use std::collections::{BTreeMap, HashSet};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    access::{AccessMode, AccessSettings},
    model::Project,
};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectDefaults {
    pub agent_id: Option<String>,
    pub root_id: Option<String>,
    pub agents: BTreeMap<String, AccessSettings>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LaunchDefaults {
    pub revision: u64,
    pub preferred_agent: Option<String>,
    pub agents: BTreeMap<String, AccessSettings>,
    pub projects: BTreeMap<String, ProjectDefaults>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultsScope {
    Agent,
    Project,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveDefaults {
    pub scope: DefaultsScope,
    pub agent_id: String,
    pub project_id: Option<String>,
    pub root_id: Option<String>,
    pub access: Option<AccessSettings>,
    pub expected_revision: u64,
    #[serde(default)]
    pub make_default_agent: bool,
}

impl LaunchDefaults {
    pub fn resolve(&self, project: &str, agent: &str, main_root: &str) -> AccessSettings {
        let mut access = self
            .projects
            .get(project)
            .and_then(|p| p.agents.get(agent))
            .or_else(|| self.agents.get(agent))
            .cloned()
            .unwrap_or_default();
        access.acknowledge_full_access = false;
        // Changing the launch folder must not duplicate it in additional roots.
        access.additional_root_ids.retain(|id| id != main_root);
        access
    }

    pub fn without_acknowledgements(mut self) -> Self {
        for access in self.agents.values_mut().chain(
            self.projects
                .values_mut()
                .flat_map(|p| p.agents.values_mut()),
        ) {
            access.acknowledge_full_access = false;
        }
        self
    }

    pub fn updated(&self, request: &SaveDefaults, project: Option<&Project>) -> Result<Self> {
        ensure!(
            self.revision == request.expected_revision,
            "Настройки запуска уже изменены. Обновите данные перед сохранением"
        );
        ensure!(
            !crate::access::modes(&request.agent_id).is_empty(),
            "Профиль агента недоступен"
        );
        let mut next = self.clone().without_acknowledgements();
        match request.scope {
            DefaultsScope::Agent => {
                ensure!(
                    request.project_id.is_none() && request.root_id.is_none(),
                    "Общие настройки агента не содержат проект или папку"
                );
                if let Some(access) = &request.access {
                    ensure!(
                        access.additional_root_ids.is_empty(),
                        "Дополнительные папки сохраняются только для конкретного проекта"
                    );
                    next.agents.insert(
                        request.agent_id.clone(),
                        validate(&request.agent_id, access, &[])?,
                    );
                } else {
                    next.agents.remove(&request.agent_id);
                }
                if request.make_default_agent {
                    next.preferred_agent = Some(request.agent_id.clone());
                }
            }
            DefaultsScope::Project => {
                let project = project.context("Проект не найден или удалён")?;
                ensure!(
                    request.project_id.as_deref() == Some(&project.id),
                    "Проект настроек не совпадает"
                );
                let existing = next.projects.get(&project.id).cloned().unwrap_or_default();
                let main = request
                    .root_id
                    .as_deref()
                    .or(existing.root_id.as_deref())
                    .or_else(|| project.roots.first().map(|r| r.id.as_str()))
                    .context("У проекта нет рабочей папки")?;
                ensure!(
                    project.roots.iter().any(|r| r.id == main),
                    "Рабочая папка больше не входит в проект"
                );
                let validated = if let Some(access) = &request.access {
                    let mut seen = HashSet::new();
                    let mut paths = Vec::new();
                    for id in &access.additional_root_ids {
                        ensure!(
                            id != main && seen.insert(id),
                            "Дополнительные папки не должны повторяться или совпадать с рабочей папкой"
                        );
                        paths.push(
                            project
                                .roots
                                .iter()
                                .find(|r| &r.id == id)
                                .context("Дополнительная папка больше не входит в проект")?
                                .path
                                .clone(),
                        );
                    }
                    Some(validate(&request.agent_id, access, &paths)?)
                } else {
                    None
                };
                // A reset of an absent preset alone need not create a project preference.
                if validated.is_some()
                    || request.root_id.is_some()
                    || request.make_default_agent
                    || next.projects.contains_key(&project.id)
                {
                    let entry = next.projects.entry(project.id.clone()).or_default();
                    if let Some(root) = &request.root_id {
                        entry.root_id = Some(root.clone());
                    }
                    if request.make_default_agent {
                        entry.agent_id = Some(request.agent_id.clone());
                    }
                    if let Some(access) = validated {
                        entry.agents.insert(request.agent_id.clone(), access);
                    } else {
                        entry.agents.remove(&request.agent_id);
                    }
                }
            }
        }
        next.revision = self
            .revision
            .checked_add(1)
            .filter(|n| *n <= i64::MAX as u64)
            .context("Исчерпан счётчик настроек запуска")?;
        Ok(next)
    }

    /// Called inside the same transaction as the project root edit.
    pub fn prune_roots(&mut self, project: &Project) -> bool {
        let before = self.clone();
        if let Some(p) = self.projects.get_mut(&project.id) {
            if p.root_id
                .as_ref()
                .is_some_and(|id| !project.roots.iter().any(|r| &r.id == id))
            {
                p.root_id = None;
            }
            for access in p.agents.values_mut() {
                access
                    .additional_root_ids
                    .retain(|id| project.roots.iter().any(|r| &r.id == id));
            }
        }
        *self != before
    }
}

fn validate(agent: &str, access: &AccessSettings, paths: &[String]) -> Result<AccessSettings> {
    let mut checked = access.clone();
    // Saving a mode is not approval to use it. The launch still needs fresh acknowledgement.
    if checked.mode == AccessMode::FullAccess {
        checked.acknowledge_full_access = true;
    }
    crate::access::launch_args_for_session(agent, &checked, paths, &crate::model::id())?;
    checked.acknowledge_full_access = false;
    Ok(checked)
}
