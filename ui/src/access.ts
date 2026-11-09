import { t } from './i18n.ts'
import type { AccessMode, Agent, Root, SessionAccess } from './api'

export function newAccess(mode: AccessMode = 'inherit'): SessionAccess {
  return { mode, approval: 'on_request', network_access: false, additional_root_ids: [], acknowledge_full_access: false }
}

export function accessControls(agent: Agent | undefined, mode: AccessMode) {
  const catalog = agent?.access_modes
  const selected = catalog?.find(option => option.id === mode)
  return {
    hasCatalog: !!catalog?.length,
    selected,
    approval: agent?.id === 'codex' && (mode === 'workspace_write' || mode === 'full_access'),
    network: agent?.id === 'codex' && mode === 'workspace_write' && agent.supports_network_control === true,
    extras: agent?.supports_extra_roots === true && (
      (agent.id === 'codex' && mode === 'workspace_write') ||
      (agent.id === 'claude' && ['manual', 'plan', 'accept_edits'].includes(mode))
    ),
    unrestricted: mode === 'full_access' || selected?.unrestricted === true,
  }
}

/** Only currently applicable controls cross the launch boundary. The daemon
 * independently validates this request and resolves root IDs to actual paths. */
export function accessRequest(agent: Agent, roots: Root[], mainRoot: string, draft: SessionAccess): SessionAccess | undefined {
  const controls = accessControls(agent, draft.mode)
  if (!controls.hasCatalog) return undefined
  if (!controls.selected) throw new Error(t("Этот режим доступа больше недоступен. Выберите другой режим."))
  if (controls.unrestricted && !draft.acknowledge_full_access) throw new Error(t("Подтвердите запуск с широкими правами агента."))
  const extraIds = controls.extras ? [...new Set(draft.additional_root_ids)] : []
  if (extraIds.some(id => id === mainRoot || !roots.some(root => root.id === id))) {
    throw new Error(t("Набор папок изменился. Повторно выберите дополнительные папки проекта."))
  }
  return {
    mode: draft.mode,
    ...(controls.approval ? { approval: draft.approval || 'on_request' } : {}),
    ...(controls.network ? { network_access: draft.network_access === true } : {}),
    additional_root_ids: extraIds,
    acknowledge_full_access: controls.unrestricted && draft.acknowledge_full_access,
  }
}
