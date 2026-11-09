import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import type { Session } from './api'
import { errorMessage, isRunning, statusLabel } from './api'
import { activitySummary, activityTime, backgroundSummary, backgroundInventoryLabel, failedBackgroundJobs, passiveBackground, toolCallLabel, unreadResult } from './activity'
import type { ActivityJob } from './activity-types'
import Modal from './Modal'
import './activity.css'

const jobLabels: Record<ActivityJob['status'], string> = { running: 'Выполнялась на момент наблюдения', scheduled: 'Запланирована', completed: 'Завершена', failed: 'Ошибка', unknown: 'Статус неизвестен' }

export default function ActivitySummary({ session, compact = false, dense = false, expanded = false, tools, onToggleExpanded, onMarkRead, onBackgroundOpenChange }: { session: Session; compact?: boolean; dense?: boolean; expanded?: boolean; tools?: ReactNode; onToggleExpanded?: () => void; onMarkRead?: (expectedRevision: number) => Promise<void>; onBackgroundOpenChange?: (open: boolean) => void }) {
  const [showBackground, setShowBackground] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const scope = useRef(session.id); scope.current = session.id
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  useEffect(() => { setShowBackground(false); setError(''); setBusy(false) }, [session.id])
  useEffect(() => () => onBackgroundOpenChange?.(false), [onBackgroundOpenChange])
  const changeBackground = (open: boolean) => { setShowBackground(open); onBackgroundOpenChange?.(open) }
  const summary = activitySummary(session), background = backgroundSummary(session.activity)
  const toolLabel = toolCallLabel(session), inventory = backgroundInventoryLabel(session)
  const passive = passiveBackground(session)
  const compactLabel = isRunning(session) ? summary.label : statusLabel(session.status)
  const compactError = !session.activity?.attention && (['failed', 'interrupted'].includes(session.status) || failedBackgroundJobs(session).length > 0)
  const markRead = async () => {
    if (!onMarkRead || busy || !unreadResult(session) || !session.activity) return
    const id = session.id, revision = session.activity.result.revision
    setBusy(true); setError('')
    try { await onMarkRead(revision) }
    catch (failure) { if (alive.current && scope.current === id) setError(errorMessage(failure)) }
    finally { if (alive.current && scope.current === id) setBusy(false) }
  }
  if (session.agent === 'shell' || session.agent === 'command') return <div className="activity-compact-toolbar"><div className={`activity-compact-row activity-${summary.tone}`} title={summary.detail}><i className="activity-state-dot" /><span className="activity-compact-copy"><strong>{summary.label}</strong></span>{session.exit_code != null && <span>exit {session.exit_code}</span>}</div>{tools}{compact && onToggleExpanded && <button type="button" className="activity-compact-expand" aria-pressed={expanded} aria-label={expanded ? t('Вернуть панели') : t('Развернуть терминал')} onClick={onToggleExpanded}>{expanded ? '⤡' : '⤢'}</button>}</div>
  return <>
    {compact || dense ? <div className={`activity-compact-toolbar ${dense ? 'activity-dense-toolbar' : ''}`}><button type="button" className={`activity-compact-row activity-${summary.tone} ${passive ? 'activity-compact-background' : ''} ${compactError ? 'activity-compact-error' : ''}`} aria-label={t('Активность: {state}. Открыть подробности', { state: [compactLabel, toolLabel, inventory, unreadResult(session) ? t('Новый результат') : ''].filter(Boolean).join(' · ') })} onClick={() => changeBackground(true)}>
      <i className="activity-state-dot" aria-hidden="true" />{dense && <span className="activity-cli-status" title={`CLI: ${statusLabel(session.status)}`}>{statusLabel(session.status)}{session.exit_code != null ? ` · exit ${session.exit_code}` : ''}</span>}<span className="activity-compact-copy"><strong>{compactLabel}</strong>{toolLabel && <small>{toolLabel}</small>}</span>
      {unreadResult(session) && <span className="activity-compact-unread" title={t('Новый результат')} aria-hidden="true">●</span>}
      {session.activity?.background.jobs.length ? <span className="activity-compact-count" title={inventory}>◷ {session.activity.background.jobs.length}</span> : null}<span className="activity-compact-chevron" aria-hidden="true">›</span>
    </button>{tools}{compact && onToggleExpanded && <button type="button" className="activity-compact-expand" aria-pressed={expanded} aria-label={expanded ? t('Вернуть панели') : t('Развернуть терминал')} onClick={onToggleExpanded}>{expanded ? '⤡' : '⤢'}</button>}</div> : <div className="activity-summary" aria-label={t("Состояние агента")}>
      <span className={`activity-state activity-${summary.tone}`} title={summary.detail}><i className="activity-state-dot" />{summary.label}</span>
      {session.activity?.attention?.certainty === 'requested' && <small>{t("Проверьте терминал")}</small>}
      {session.activity?.phase === 'working' && summary.tone !== 'working' && <small>{t("Агент выполняет задачу")}</small>}
      {session.activity && session.activity.updated_at === null && session.activity.phase === 'unknown' && Object.values(session.activity.capabilities).some(Boolean) && <small>{t("События CLI ещё не поступали")}</small>}
      {session.activity?.adapter === 'codex_hooks' && session.activity.updated_at === null && <small>{t("Подтвердите hooks в CLI, если он запросит разрешение.")}</small>}
      {unreadResult(session) && summary.tone !== 'result' && !passive && <small>{t("Есть новый результат")}</small>}
      {unreadResult(session) && !passive && <button type="button" className="activity-read" onClick={() => void markRead()} disabled={!onMarkRead || busy}>{busy ? t("Сохраняем…") : t("Отметить просмотренным")}</button>}
      {toolLabel && <span className="activity-tool-call" title={t('Вызов отмечен CLI. Он может ожидать разрешения; это не подтверждение исполнения.')}>{toolLabel}</span>}
      <button type="button" className="activity-background" onClick={() => changeBackground(true)} title={t('Открыть активность и фоновые задачи')}>{inventory || t('Активность и фон')}</button>
    </div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    {showBackground && <Modal title={t('Активность и фоновые задачи')} subtitle={session.title} onClose={() => changeBackground(false)}>
      <section className="activity-current-details" aria-label={t('Текущая активность')}>
        <h3>{t('Текущая активность')}</h3><strong className={`activity-${summary.tone}`}>{summary.label}</strong>
        <p>CLI: {statusLabel(session.status)}{session.exit_code != null ? ` · exit ${session.exit_code}` : ''}</p>
        <p>{t('Shift+Enter — новая строка · Enter — отправить')}{t(' · ⌘/Ctrl + клик — файл')}</p>
        {session.environment && <p className="session-environment-details"><strong>{t('Файл ENV')}</strong>{' '}<code>{session.environment.file}</code>{' · '}{t('Переменных: {count}', { count: session.environment.variable_count })}</p>}
        {session.activity?.attention?.certainty === 'requested' && <p>{t('Проверьте терминал')}</p>}
        {session.activity && session.activity.updated_at === null && <p>{t('События CLI ещё не поступали')}</p>}
        {session.activity?.adapter === 'codex_hooks' && session.activity.updated_at === null && <p>{t('Подтвердите hooks в CLI, если он запросит разрешение.')}</p>}
        {session.activity?.phase === 'working' && summary.tone !== 'working' && <p>{t('Агент продолжает выполнять задачу')}</p>}
        {session.activity?.tools_observed_at != null && ['running', 'needs_attention'].includes(session.status)
          ? <>{session.activity.tools?.length ? <><p>{t('Вызов отмечен CLI. Он может ожидать разрешения; это не подтверждение исполнения.')}</p><ul>{session.activity.tools.map(tool => <li key={tool.id}><strong>{tool.name}</strong><small>{t('Наблюдение:')} {activityTime(tool.started_at)}</small></li>)}</ul></> : <p>{t('Незавершённые вызовы инструментов не зафиксированы.')}</p>}</>
          : <p>{t('Подробности вызовов инструментов для этого запуска недоступны.')}</p>}
        <small>{t('Последнее событие агента: {time}', { time: activityTime(session.activity?.updated_at) })}</small>
        {(passive || compact || dense) && unreadResult(session) && <div className="activity-optional-result"><small>{passive ? t('Есть новый результат. Просмотр необязателен и не требует действия для работы монитора.') : t('Есть новый результат')}</small><button type="button" className="activity-read" onClick={() => void markRead()} disabled={!onMarkRead || busy}>{busy ? t('Сохраняем…') : t('Отметить просмотренным')}</button></div>}
        {error && <div className="inline-error" role="alert">{t(error)}</div>}
      </section>
      <h3>{t('Фоновые задачи и расписания')}</h3>
      <div className="activity-background-info"><strong>{background.label}</strong><p>{background.detail}</p><p>{t("Наблюдение:")}{' '}{activityTime(session.activity?.background.observed_at)}</p></div>
      {background.showJobs && session.activity?.background.coverage !== 'snapshot' && <p className="activity-previous-observation">{t('Ниже — ранее полученные записи. Список может быть неполным; текущая работоспособность не подтверждена.')}</p>}
      {background.showJobs && <div className="activity-jobs">{session.activity!.background.jobs.length === 0 ? <p>{t("В полученном снимке задач нет.")}</p> : session.activity!.background.jobs.map(job => <article key={job.id} className="activity-job"><strong>{job.name || job.id}</strong><span>{t(jobLabels[job.status] || jobLabels.unknown)}</span><small>{job.kind || t("Тип неизвестен")}{' '}{t("· Наблюдение:")}{' '}{activityTime(job.observed_at)}</small>{job.next_run_at !== null && <small>{t("Следующий запуск по данным агента:")}{' '}{activityTime(job.next_run_at)}</small>}</article>)}</div>}
      <div className="modal-actions"><button type="button" className="button" onClick={() => changeBackground(false)}>{t("Закрыть")}</button></div>
    </Modal>}
  </>
}
