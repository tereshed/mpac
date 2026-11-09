import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { api, errorMessage } from './api'
import type { Agent, LaunchDefaults, Project } from './api'
import Modal from './Modal'
import AccessSettings from './AccessSettings'
import { accessControls } from './access'
import { defaultsSaveRequest, presetSourceLabel, resolveLaunchPreset } from './launch-defaults'
import type { DefaultsScope, PresetSource } from './launch-defaults'
import './launch-defaults.css'

interface Props {
  token: string; project?: Project; agents: Agent[]; defaults: LaunchDefaults;
  onClose: () => void; onSaved: (catalog: LaunchDefaults) => boolean;
}

const GLOBAL_PROJECT: Project = { id: 'global-launch-settings', name: '', roots: [] }

function settingsAgent(agents: Agent[], defaults: LaunchDefaults, project?: Project): string {
  const candidates = [project ? defaults.projects[project.id]?.agent_id : null, defaults.preferred_agent, 'demo', ...agents.map(agent => agent.id)]
  return candidates.find(id => agents.some(agent => agent.id === id && agent.access_modes?.length)) || ''
}

function settingsRoot(project: Project | undefined, defaults: LaunchDefaults, fallback = ''): string {
  if (!project) return ''
  return [defaults.projects[project.id]?.root_id, fallback].find(id => project.roots.some(root => root.id === id)) || project.roots[0]?.id || ''
}

export default function LaunchDefaultsDialog(props: Props) {
  return <DefaultsForm key={props.token + '\0' + (props.project?.id || 'global')} {...props} />
}

function DefaultsForm({ token, project, agents, defaults, onClose, onSaved }: Props) {
  const [catalog, setCatalog] = useState(defaults)
  const [scope, setScope] = useState<DefaultsScope>(project ? 'project' : 'agent')
  const [agentId, setAgentId] = useState(() => settingsAgent(agents, defaults, project))
  const [rootId, setRootId] = useState(() => settingsRoot(project, defaults))
  const [initial] = useState(() => resolveLaunchPreset(agents.find(agent => agent.id === agentId), project, rootId, defaults))
  const [access, setAccess] = useState(initial.access)
  const [source, setSource] = useState<PresetSource>(initial.source)
  const preferred = (nextScope: DefaultsScope, nextCatalog: LaunchDefaults) => nextScope === 'project' && project ? nextCatalog.projects[project.id]?.agent_id : nextCatalog.preferred_agent
  const [makeDefault, setMakeDefault] = useState(() => preferred(scope, defaults) === agentId)
  const [busy, setBusy] = useState<'save' | 'reset' | null>(null)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const pending = useRef(false)
  const mounted = useRef(true)
  const request = useRef<AbortController | null>(null)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; request.current?.abort() } }, [])

  const agent = agents.find(item => item.id === agentId)
  const editableAgents = agents.filter(item => item.access_modes?.length)
  const controls = accessControls(agent, access.mode)
  const currentPreferred = preferred(scope, catalog) === agentId
  const scopedProject = scope === 'project' ? project : undefined
  const rootValid = scope !== 'project' || !!project?.roots.some(root => root.id === rootId)
  const valid = !!agent && !!controls.selected && rootValid
  const ownPreset = scope === 'project' && project ? catalog.projects[project.id]?.agents[agentId] : catalog.agents[agentId]
  const newerCatalog = defaults.revision > catalog.revision

  const loadDraft = (nextScope: DefaultsScope, nextAgent: string, nextRoot: string, nextCatalog = catalog) => {
    const preset = resolveLaunchPreset(agents.find(item => item.id === nextAgent), nextScope === 'project' ? project : undefined, nextRoot, nextCatalog)
    setAccess(preset.access); setSource(preset.source); setMakeDefault(preferred(nextScope, nextCatalog) === nextAgent); setError(''); setNotice('')
  }
  const changeScope = (nextScope: DefaultsScope) => {
    const id = settingsAgent(agents, catalog, nextScope === 'project' ? project : undefined)
    const root = settingsRoot(nextScope === 'project' ? project : undefined, catalog, rootId)
    setScope(nextScope); setAgentId(id); setRootId(root); loadDraft(nextScope, id, root)
  }
  const save = async (reset: boolean) => {
    if (pending.current || !agent || (!reset && !valid) || (reset && !ownPreset)) return
    let body
    try { body = defaultsSaveRequest(catalog, scope, agent, project, rootId, reset ? null : access, makeDefault) }
    catch (failure) { setError(errorMessage(failure)); return }
    const abort = new AbortController(); request.current = abort; pending.current = true
    setBusy(reset ? 'reset' : 'save'); setError(''); setNotice('')
    try {
      const saved = await api<LaunchDefaults>(token, '/launch-defaults', body, abort.signal)
      if (!mounted.current || abort.signal.aborted) return
      setCatalog(saved); loadDraft(scope, agentId, rootId, saved)
      if (!onSaved(saved)) setError(t("Настройки сохранены, но подключение изменилось. Закройте окно и откройте настройки снова."))
      else setNotice(reset ? t("Сброшены только параметры доступа этого агента. Сохранённый выбор агента и папки не изменён.") : t("Настройки запуска сохранены. Работающие сессии не изменяются."))
    } catch (failure) { if (mounted.current && !abort.signal.aborted) setError(errorMessage(failure)) }
    finally { pending.current = false; if (request.current === abort) request.current = null; if (mounted.current) setBusy(null) }
  }
  const submit = (event: FormEvent) => { event.preventDefault(); void save(false) }
  return <Modal title={t("Настройки запуска")} subtitle={project ? t("Параметры агента и проекта «{p0}».", { p0: project.name }) : t("Общие параметры агентов для будущих сессий.")} onClose={() => { if (!pending.current) onClose() }} busy={!!busy}>
    <form className="launch-defaults-form" onSubmit={submit}>
      {project && <div className="launch-defaults-scope" role="group" aria-label={t("Область настроек")}><button type="button" className={scope === 'project' ? 'active' : ''} disabled={!!busy} onClick={() => changeScope('project')}>{t("Для этого проекта")}</button><button type="button" className={scope === 'agent' ? 'active' : ''} disabled={!!busy} onClick={() => changeScope('agent')}>{t("Общие для агента")}</button></div>}
      <p className="launch-defaults-scope-help">{scope === 'project' ? t("Настройки этого проекта имеют приоритет перед общими параметрами агента.") : t("Эти параметры используются в проектах без собственных настроек этого агента.")}</p>
      <div className={scope === 'project' ? 'form-grid' : ''}><label>{t("Агент")}<select autoFocus value={agentId} disabled={!!busy || !editableAgents.length} onChange={event => { const id = event.target.value; setAgentId(id); loadDraft(scope, id, rootId) }} required>{!editableAgents.length && <option value="">{t("Нет доступных настроек")}</option>}{editableAgents.map(item => <option key={item.id} value={item.id}>{t(item.label)}{!item.available ? t(" · пока недоступен для запуска") : ''}</option>)}</select></label>{scope === 'project' && project && <label>{t("Папка для новых сессий")}<select value={rootId} disabled={!!busy} onChange={event => { const id = event.target.value; setRootId(id); loadDraft(scope, agentId, id) }} required>{project.roots.map(root => <option key={root.id} value={root.id}>{root.name}</option>)}</select></label>}</div>
      <label className="launch-default-agent"><input type="checkbox" checked={currentPreferred || makeDefault} disabled={!!busy || currentPreferred || !agent} onChange={event => setMakeDefault(event.target.checked)} /><span>{currentPreferred ? t("Этот агент уже выбран по умолчанию") : scope === 'project' ? t("Сделать агентом по умолчанию для проекта") : t("Сделать агентом по умолчанию")}<small>{currentPreferred ? t("Чтобы изменить выбор, настройте другого агента и отметьте его.") : t("Без этой отметки прежний выбор агента сохраняется.")}</small></span></label>
      <div className="launch-preset-source"><span>{t("Доступ:")}</span><strong>{source === 'custom' ? t("Изменяется сохранённый шаблон") : presetSourceLabel(source)}</strong></div>
      <AccessSettings agent={agent} project={scopedProject || GLOBAL_PROJECT} mainRoot={scope === 'project' ? rootId : ''} value={access} onChange={value => { setAccess({ ...value, acknowledge_full_access: false }); setSource('custom'); setError('') }} disabled={!!busy} acknowledgementRequired={false} allowExtraRoots={scope === 'project'} />
      {scope === 'agent' && <p className="launch-defaults-footnote">{t("Дополнительные папки задаются отдельно в настройках конкретного проекта.")}</p>}
      <p className="launch-defaults-footnote">{t("Настройки применяются только к будущим запускам. Перед запуском их можно изменить для одной сессии.")}</p>
      {newerCatalog && <div className="notice">{t("Настройки изменились в другом окне. Ваш черновик не перезаписан.")}<button className="text-button" type="button" disabled={!!busy} onClick={() => { setCatalog(defaults); loadDraft(scope, agentId, rootId, defaults) }}>{t("Загрузить актуальные параметры")}</button></div>}
      {notice && <div className="success-notice" role="status">{t(notice)}</div>}
      {error && <div className="inline-error" role="alert">{t(error)}</div>}
      <div className="launch-defaults-reset"><button type="button" disabled={!!busy || !ownPreset} onClick={() => void save(true)}>{busy === 'reset' ? t("Сбрасываем…") : t("Сбросить настройки этого агента")}</button><span>{t("Выбор агента и папки по умолчанию не сбрасывается.")}</span></div>
      <div className="modal-actions"><button type="button" className="button secondary" disabled={!!busy} onClick={onClose}>{t("Закрыть")}</button><button className="button primary" disabled={!!busy || !valid}>{busy === 'save' ? t("Сохраняем…") : t("Сохранить настройки")}</button></div>
    </form>
  </Modal>
}
