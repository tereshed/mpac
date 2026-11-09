import { t } from './i18n.ts'
import { accessControls, newAccess } from './access.ts'
import type { Agent, LaunchDefaults, Project, SessionAccess } from './api'

export type PresetSource = 'project' | 'agent' | 'inherit' | 'custom'
export type DefaultsScope = 'agent' | 'project'

export function presetSourceLabel(source: PresetSource): string {
  return ({ project: t("Настройки этого проекта"), agent: t("Общие настройки агента"), inherit: t("Текущие настройки CLI"), custom: t("Изменено только для этого запуска") })[source]
}

export function cleanPresetAccess(agent: Agent | undefined, project: Project | undefined, rootId: string, preset?: SessionAccess | null): SessionAccess {
  const mode = preset?.mode && agent?.access_modes?.some(option => option.id === preset.mode) ? preset.mode : 'inherit'
  const controls = accessControls(agent, mode)
  const allowed = new Set(project?.roots.map(root => root.id) || [])
  return {
    ...newAccess(mode),
    ...(controls.approval ? { approval: preset?.approval === 'never' ? 'never' : 'on_request' } : {}),
    network_access: controls.network && preset?.network_access === true,
    additional_root_ids: controls.extras && project ? [...new Set((preset?.additional_root_ids || []).filter(id => id !== rootId && allowed.has(id)))] : [],
    // Approval belongs to one actual launch, never a reusable preset.
    acknowledge_full_access: false,
  }
}

export function resolveLaunchPreset(agent: Agent | undefined, project: Project | undefined, rootId: string, defaults?: LaunchDefaults): { access: SessionAccess; source: PresetSource } {
  const projectPreset = agent && project ? defaults?.projects[project.id]?.agents[agent.id] : undefined
  const globalPreset = agent ? defaults?.agents[agent.id] : undefined
  return {
    access: cleanPresetAccess(agent, projectPreset ? project : undefined, rootId, projectPreset || globalPreset),
    source: projectPreset ? 'project' : globalPreset ? 'agent' : 'inherit',
  }
}

export function initialLaunchSelection(project: Project, agents: Agent[], rootId: string, defaults?: LaunchDefaults, workflowRootId?: string): { agentId: string; rootId: string; access: SessionAccess; source: PresetSource } {
  const projectDefaults = defaults?.projects[project.id]
  const candidates = [projectDefaults?.agent_id, defaults?.preferred_agent, 'demo', ...agents.map(agent => agent.id)]
  const agent = candidates.map(id => agents.find(item => item.id === id && item.available)).find(Boolean)
  const root = [workflowRootId, projectDefaults?.root_id, rootId].map(id => project.roots.find(item => item.id === id)).find(Boolean) || project.roots[0]
  const selectedRoot = root?.id || ''
  return { agentId: agent?.id || '', rootId: selectedRoot, ...resolveLaunchPreset(agent, project, selectedRoot, defaults) }
}

export function accessPresetForStorage(agent: Agent, project: Project | undefined, rootId: string, draft: SessionAccess): SessionAccess {
  const controls = accessControls(agent, draft.mode)
  if (!controls.hasCatalog || !controls.selected) throw new Error(t("Выберите режим доступа, поддерживаемый этим агентом."))
  if (project && !project.roots.some(root => root.id === rootId)) throw new Error(t("Выберите существующую папку проекта."))
  const cleaned = cleanPresetAccess(agent, project, rootId, draft)
  return {
    mode: cleaned.mode,
    ...(controls.approval ? { approval: cleaned.approval } : {}),
    ...(controls.network ? { network_access: cleaned.network_access } : {}),
    additional_root_ids: cleaned.additional_root_ids,
    acknowledge_full_access: false,
  }
}

export function defaultsSaveRequest(catalog: LaunchDefaults, scope: DefaultsScope, agent: Agent, project: Project | undefined, rootId: string, draft: SessionAccess | null, makeDefault: boolean) {
  if (scope === 'project' && !project) throw new Error(t("Выберите проект для его настроек запуска."))
  return {
    scope, agent_id: agent.id,
    ...(scope === 'project' ? { project_id: project!.id } : {}),
    access: draft === null ? null : accessPresetForStorage(agent, scope === 'project' ? project : undefined, rootId, draft),
    expected_revision: catalog.revision,
    // Reset removes this preset only; it must not change preferred choices.
    ...(draft !== null ? { ...(scope === 'project' ? { root_id: rootId } : {}), ...(makeDefault ? { make_default_agent: true } : {}) } : {}),
  }
}
