import { t } from './i18n.ts'
import type { AccessMode, Agent, Project, SessionAccess } from './api'
import { accessControls, newAccess } from './access'

export default function AccessSettings({ agent, project, mainRoot, value, onChange, disabled, acknowledgementRequired = true, allowExtraRoots = true }: {
  agent: Agent | undefined
  project: Project
  mainRoot: string
  value: SessionAccess
  onChange: (value: SessionAccess) => void
  disabled: boolean
  acknowledgementRequired?: boolean
  allowExtraRoots?: boolean
}) {
  const controls = accessControls(agent, value.mode)
  const extras = project.roots.filter(root => root.id !== mainRoot)
  const update = (change: Partial<SessionAccess>) => onChange({ ...value, ...change, acknowledge_full_access: false })

  return <section className="access-settings" aria-labelledby="access-settings-title">
    <div className="access-settings-title"><h3 id="access-settings-title">{t(agent?.id === "shell" || agent?.id === "command" ? "Доступ" : "Доступ агента")}</h3><span>{acknowledgementRequired ? t("Для этой сессии") : t("Для будущих запусков")}</span></div>
    {agent?.id === 'shell' || agent?.id === 'command' ? <div className="access-fixed"><strong>{t('Права вашего пользователя')}</strong><p>{t('Это обычный процесс оболочки. Настройки разрешений AI-агента к нему не применяются.')}</p></div> : agent?.id === 'demo' ? <div className="access-fixed"><strong>{t("Фиксированный демо-сценарий")}</strong><p>{t("Демо не запускает модель и не выполняет произвольные команды. Дополнительные разрешения не требуются.")}</p></div> : !controls.hasCatalog ? <div className="access-fixed"><strong>{t("Текущие настройки CLI")}</strong><p>{t("Локальный daemon пока не сообщает доступные режимы. Сессия использует настройки оригинального CLI; выбор прав появится после обновления приложения.")}</p></div> : <>
      <label className="access-mode-label" htmlFor="agent-access-mode">{t("Режим работы")}<select id="agent-access-mode" disabled={disabled} value={value.mode} aria-describedby="access-mode-description" onChange={event => onChange(newAccess(event.target.value as AccessMode))}>{agent?.access_modes?.map(mode => <option key={mode.id} value={mode.id}>{t(mode.label)}</option>)}</select></label>
      {controls.selected && <p className={`access-description ${controls.unrestricted ? 'access-description-full' : ''}`} id="access-mode-description">{t(controls.selected.description)}</p>}
      {controls.approval && <div className="access-control"><label htmlFor="agent-access-approval">{t("Подтверждение действий")}<select id="agent-access-approval" disabled={disabled} value={value.approval || 'on_request'} onChange={event => update({ approval: event.target.value as 'on_request' | 'never' })}><option value="on_request">{t("Спрашивать при необходимости")}</option><option value="never">{t("Не запрашивать подтверждения")}</option></select></label><p>{t("Отключение запросов само по себе не расширяет доступ к файлам.")}</p></div>}
      {controls.network && <div className="access-control"><label className="access-checkbox"><input type="checkbox" disabled={disabled} checked={value.network_access === true} onChange={event => update({ network_access: event.target.checked })} /><span><strong>{t("Сеть для команд")}</strong><small>{t("Управляет сетью команд в выбранном режиме CLI. Подключение к самой модели не меняется.")}</small></span></label></div>}
      {controls.extras && allowExtraRoots && <fieldset className="access-extra-roots" disabled={disabled}><legend>{t("Дополнительные папки проекта")}</legend><p>{t("Основная папка уже включена. Доступ к остальным папкам задаётся выбранным режимом.")}</p>{extras.length ? extras.map(root => <label className="access-checkbox access-root-choice" key={root.id}><input type="checkbox" checked={value.additional_root_ids.includes(root.id)} onChange={event => update({ additional_root_ids: event.target.checked ? [...value.additional_root_ids, root.id] : value.additional_root_ids.filter(id => id !== root.id) })} /><span><strong>{root.name}</strong><code title={root.path}>{root.path}</code></span></label>) : <div className="access-no-extras">{t("В этом проекте пока нет других папок.")}</div>}</fieldset>}
      {controls.unrestricted && (acknowledgementRequired ? <label className="access-checkbox access-full-ack"><input type="checkbox" disabled={disabled} required checked={value.acknowledge_full_access} onChange={event => onChange({ ...value, acknowledge_full_access: event.target.checked })} /><span><strong>{t("Подтверждаю запуск с широкими правами")}</strong><small>{t("Разрешаю выбранный режим CLI. Действия агента могут затронуть файлы вне проекта, доступные моему пользователю.")}</small></span></label> : <div className="access-preset-full"><strong>{t("Подтверждение потребуется при каждом запуске")}</strong><p>{t("Сохраняется только режим доступа. Разрешение на запуск с широкими правами в настройках не запоминается.")}</p></div>)}
    </>}
  </section>
}
