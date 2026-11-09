import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { api, errorMessage } from './api'
import type { Agent, LaunchDefaults, Project, Session } from './api'
import Modal from './Modal'
import AccessSettings from './AccessSettings'
import EnvFilePicker from './EnvFilePicker'
import './session-environment.css'
import { accessControls, accessRequest } from './access'
import { initialLaunchSelection, presetSourceLabel, resolveLaunchPreset } from './launch-defaults'
import type { PresetSource } from './launch-defaults'
import './launch-defaults.css'

interface Props {
  token: string; project: Project; agents: Agent[]; rootId: string; defaults?: LaunchDefaults; autoTitleAvailable?: boolean; environmentFilesAvailable?: boolean;
  initialRootId?: string; requiredRootIds?: string[];
  onClose: () => void; onLaunched: (session: Session) => void;
}

export default function LaunchSession(props: Props) {
  return <LaunchSessionForm key={props.token + '\0' + props.project.id} {...props} />
}

function LaunchSessionForm({ token, project, agents, rootId, defaults, autoTitleAvailable = false, environmentFilesAvailable = false, initialRootId, requiredRootIds = [], onClose, onLaunched }: Props) {
  const [initial] = useState(() => initialLaunchSelection(project, agents, rootId, defaults, initialRootId))
  const [agentId, setAgentId] = useState(initial.agentId)
  const [selectedRoot, setSelectedRoot] = useState(initial.rootId)
  const [title, setTitle] = useState('')
  const [command, setCommand] = useState('')
  const [environmentEnabled, setEnvironmentEnabled] = useState(false)
  const [environmentFile, setEnvironmentFile] = useState('')
  const [pickingEnvironment, setPickingEnvironment] = useState(false)
  const [access, setAccess] = useState(initial.access)
  const [source, setSource] = useState<PresetSource>(initial.source)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const mounted = useRef(true)
  const pending = useRef(false)
  const request = useRef<AbortController | null>(null)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; request.current?.abort() } }, [])
  const agent = agents.find(item => item.id === agentId)
  const terminalLaunch = agentId === 'shell' || agentId === 'command'
  const commandValid = agentId !== 'command' || (!!command.trim() && new TextEncoder().encode(command).length <= 16384 && !command.includes('\0'))
  const controls = accessControls(agent, access.mode)
  const rootValid = project.roots.some(item => item.id === selectedRoot)
  const accessValid = !controls.hasCatalog || (!!controls.selected && (!controls.unrestricted || access.acknowledge_full_access))
  const usingEnvironment = agentId !== 'demo' && environmentFilesAvailable && environmentEnabled
  const environmentValid = agentId === 'demo' || !environmentEnabled || (environmentFilesAvailable && environmentFile.length > 0)
  const applyPreset = (nextAgent: string, nextRoot: string) => {
    const preset = resolveLaunchPreset(agents.find(item => item.id === nextAgent), project, nextRoot, defaults)
    setAccess(preset.access); setSource(preset.source); setError('')
  }
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (pending.current || !agent?.available || !rootValid || !accessValid || !environmentValid || !commandValid) return
    pending.current = true; setBusy(true); setError('')
    const abort = new AbortController(); request.current = abort
    try {
      const session = await api<Session>(token, '/sessions', { project_id: project.id, root_id: selectedRoot, agent: agentId, ...(agentId === 'command' ? { command } : {}), title: title.trim() || undefined, ...(usingEnvironment ? { env_file: environmentFile } : {}), access: accessRequest(agent, project.roots, selectedRoot, access) }, abort.signal)
      if (mounted.current && !abort.signal.aborted) onLaunched(session)
    } catch (failure) { if (mounted.current && !abort.signal.aborted) setError(errorMessage(failure)) }
    finally { pending.current = false; if (mounted.current) setBusy(false); if (request.current === abort) request.current = null }
  }
  if (pickingEnvironment) return <EnvFilePicker token={token} initialPath={project.roots.find(root => root.id === selectedRoot)?.path || ''} onClose={() => setPickingEnvironment(false)} onSelect={path => { setEnvironmentFile(path); setEnvironmentEnabled(true); setPickingEnvironment(false); setError('') }} />
  return <Modal title={t("Новая сессия")} subtitle={project.name + t(" · локальный запуск CLI")} onClose={() => { if (!pending.current) onClose() }} busy={busy}><form onSubmit={submit}>
    <div className="form-grid"><label>{t("Запуск")}<select autoFocus disabled={busy} value={agentId} onChange={event => { const id = event.target.value; setAgentId(id); applyPreset(id, selectedRoot) }} required>{[false, true].map(terminal => <optgroup key={String(terminal)} label={t(terminal ? "Терминал и команды" : "AI-агенты")}>{agents.filter(item => (["shell", "command"].includes(item.id)) === terminal).map(item => <option key={item.id} value={item.id} disabled={!item.available}>{t(item.label)}{!item.available ? t(" · недоступен") : ''}</option>)}</optgroup>)}</select></label><label>{t("Рабочая папка")}<select disabled={busy} value={selectedRoot} onChange={event => { const id = event.target.value; setSelectedRoot(id); applyPreset(agentId, id) }} required>{project.roots.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label></div>
    {agentId === 'command' && <label>{t("Команда запуска")}<textarea value={command} onChange={event => setCommand(event.target.value)} placeholder="npm run dev" rows={3} maxLength={16384} disabled={busy} required autoComplete="off" spellCheck={false} /><span className="launch-auto-title-help">{t("Можно использовать аргументы, пайпы и несколько строк. Сессия завершится вместе с командой.")}</span></label>}
    <label>{t("Название сессии")}{' '}<span className="muted">{t("необязательно")}</span><input value={title} onChange={event => setTitle(event.target.value)} placeholder={t("Например, Ревью авторизации")} maxLength={100} disabled={busy} /></label>
    {autoTitleAvailable && !title.trim() && ['claude', 'codex', 'zai'].includes(agentId) && <p className="launch-auto-title-help">{t("Если название пустое, получим его из CLI после первого завершённого ответа, когда тот его передаст.")}</p>}
    {!title.trim() && agentId === 'hermes' && <p className="launch-auto-title-help">{t("Название можно задать вручную; автоматическое имя этого CLI пока недоступно.")}</p>}
    {agentId !== 'demo' && <section className="session-environment" aria-label={t('Окружение сессии')}>
      <label className="session-environment-toggle"><input type="checkbox" checked={environmentEnabled} disabled={busy || (!environmentFilesAvailable && !environmentEnabled)} onChange={event => { setEnvironmentEnabled(event.target.checked); setError('') }} />{t('Загрузить ENV из файла')}</label>
      {!environmentFilesAvailable && <p className="muted">{t('Загрузка ENV станет доступна после обновления локального сервиса.')}</p>}
      {usingEnvironment && <><label htmlFor="session-env-file">{t('Файл ENV')}</label><div className="session-environment-path"><input id="session-env-file" value={environmentFile} onChange={event => { setEnvironmentFile(event.target.value); setError('') }} placeholder={t("Например, config/agent.env")} maxLength={4096} autoComplete="off" spellCheck={false} disabled={busy} /><button type="button" className="button secondary compact" disabled={busy} onClick={() => setPickingEnvironment(true)}>{t('Выбрать файл…')}</button></div><p>{t('Абсолютный путь или путь от рабочей папки. Поддерживаются KEY=value и export KEY=value.')}</p><p>{t('Переменные действуют только в новой сессии. Содержимое файла не сохраняется в истории и не показывается в интерфейсе.')}</p></>}
    </section>}
    {!terminalLaunch && <><div className="launch-preset-source"><span>{t("Доступ:")}</span><strong>{presetSourceLabel(source)}</strong></div>
    {!!requiredRootIds.length && <p className="launch-auto-title-help">{t("Папки выбранного ревью:")}{' '}{project.roots.filter(root => requiredRootIds.includes(root.id)).map(root => root.name).join(', ')}{t(". Проверьте доступ к ним перед запуском. Пакет замечаний будет подготовлен после выбора новой сессии.")}</p>}
    <AccessSettings agent={agent} project={project} mainRoot={selectedRoot} value={access} onChange={value => { setAccess(value); setSource('custom'); setError('') }} disabled={busy} /></>}
    <div className="billing-card"><span className="billing-icon">◇</span><div><strong>{terminalLaunch ? t("Системная оболочка") : agentId === 'demo' ? t("Демо без обращений к моделям") : usingEnvironment ? t("Авторизация CLI с ENV-файлом") : t("Авторизация оригинального CLI")}</strong><p>{terminalLaunch ? t(agentId === "shell" ? "Откроется интерактивный терминал в выбранной папке. Агент автоматически не запускается." : "Команда выполняется вашей оболочкой в выбранной папке.") : usingEnvironment ? t("Провайдер, модель и способ оплаты могут быть заданы переменными этого файла.") : agent?.billing ? t(agent.billing) : t("Условия оплаты неизвестны.")}</p>{!terminalLaunch && agentId !== 'demo' && !usingEnvironment && <p>{t("Автоматического переключения на API нет. Возможные доплаты на стороне провайдера не проверены.")}</p>}</div></div>
    {!rootValid && <div className="notice">{t("Набор папок проекта изменился. Выберите рабочую папку повторно.")}</div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={busy}>{t("Отмена")}</button><button className="button primary" disabled={busy || !agent?.available || !rootValid || !accessValid || !environmentValid || !commandValid}>{busy ? t("Запускаем…") : t("Запустить сессию →")}</button></div>
  </form></Modal>
}
