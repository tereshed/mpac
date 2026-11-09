import { localeTag, t } from './i18n.ts'
import type { ActivityState } from './activity-types'

export interface ActivitySession { agent?: string; exit_code?: number | null; status: string; activity?: ActivityState }
export interface ActivityIndicator { label: string; tone: 'attention' | 'result' | 'working' | 'idle' | 'unknown' | 'stopped'; detail: string }
const processAlive = (session: ActivitySession) => ['running', 'needs_attention'].includes(session.status)
const legacyAttention = (session: ActivitySession) => session.status === 'needs_attention' && (!session.activity || (session.activity.adapter === 'legacy' && !session.activity.capabilities.foreground))

export function unreadResult(session: ActivitySession): boolean {
  const result = session.activity?.result
  return !!result && Number.isSafeInteger(result.revision) && Number.isSafeInteger(result.read_revision) && result.revision > 0 && result.read_revision >= 0 && result.revision > result.read_revision
}
export function failedBackgroundJobs(session: ActivitySession) {
  return session.activity?.background.coverage === 'snapshot' ? session.activity.background.jobs.filter(job => job.status === 'failed') : []
}
export function backgroundWorkSummary(session: ActivitySession): ActivityIndicator | null {
  const background = session.activity?.background
  if (!processAlive(session) || background?.coverage !== 'snapshot') return null
  if (background.jobs.some(job => job.status === 'running')) return { label: t('Фоновая работа · снимок'), tone: 'working', detail: t('В последнем снимке есть работающая фоновая задача. Само по себе это не требует действия.') }
  if (background.jobs.some(job => job.status === 'scheduled')) return { label: t('По расписанию · снимок'), tone: 'idle', detail: t('В последнем снимке есть задания по расписанию. Само по себе это не требует действия.') }
  return null
}
export function passiveBackground(session: ActivitySession): boolean {
  return !!backgroundWorkSummary(session) && session.activity?.phase !== 'working' && !session.activity?.attention && !legacyAttention(session) && !failedBackgroundJobs(session).length
}
export function activitySummary(session: ActivitySession): ActivityIndicator {
  if (session.agent === 'shell' || session.agent === 'command') {
    if (processAlive(session)) return { label: t(session.agent === 'shell' ? 'Терминал открыт' : 'Выполняется команда'), tone: session.agent === 'shell' ? 'idle' : 'working', detail: t('Процесс выполняется без интеграции AI-агента.') }
    return { label: t(session.status === 'failed' ? 'Процесс завершился с ошибкой' : ['interrupted', 'stopped'].includes(session.status) ? 'Процесс остановлен' : 'Процесс завершён'), tone: session.status === 'failed' ? 'attention' : 'stopped', detail: session.exit_code == null ? '' : t('Код завершения: {code}', { code: session.exit_code }) }
  }
  const activity = session.activity, attention = activity?.attention
  if (attention) {
    const confirmed = attention.certainty === 'confirmed'
    const label = attention.kind === 'question' ? confirmed ? t("Ждёт ответа") : t("Запрошен вопрос") : attention.kind === 'permission' ? confirmed ? t("Ждёт разрешения") : t("Запрошено разрешение") : confirmed ? t("Ждёт ввода") : t("Запрошен ввод")
    return { label, tone: 'attention', detail: confirmed ? t("Откройте запрос в терминале.") : t("Проверьте терминал: запрос отмечен адаптером, ожидание ещё не подтверждено.") }
  }
  if (legacyAttention(session)) return { label: t("Нужно внимание"), tone: 'attention', detail: t("Проверьте терминал. Подробное состояние агента недоступно.") }
  if (failedBackgroundJobs(session).length) return { label: t('Ошибка фоновой задачи в снимке'), tone: 'attention', detail: t('Откройте активность и фоновые задачи. Ошибка зафиксирована на момент наблюдения; текущее состояние нужно проверить.') }
  if (activity?.phase === 'working') return { label: t("Агент выполняет задачу"), tone: 'working', detail: t("Работа отмечена адаптером агента.") }
  const background = backgroundWorkSummary(session)
  if (background) return background
  if (unreadResult(session)) return { label: t("Новый результат"), tone: 'result', detail: t("Есть результат, который вы ещё не отметили просмотренным. Агент может продолжать работу.") }
  if (activity?.phase === 'idle') return { label: t("Агент ожидает задачу"), tone: 'idle', detail: t("Ожидание подтверждено адаптером агента.") }
  if (activity?.phase === 'stopped') return { label: t("Агент остановлен"), tone: 'stopped', detail: t("Адаптер сообщил об остановке агента.") }
  return { label: t("Статус агента неизвестен"), tone: 'unknown', detail: processAlive(session) ? t("CLI-процесс запущен. Это не подтверждает выполнение задачи моделью.") : t("CLI-процесс завершён; подробное состояние агента недоступно.") }
}
/** Priority only observes durable state. Opening tabs, output bytes and typing
 * never acknowledge a result or clear a pending question. */
export function attentionPriority(session: ActivitySession): number {
  if (session.activity?.attention || legacyAttention(session)) return 0
  if (['failed', 'interrupted'].includes(session.status) || failedBackgroundJobs(session).length) return 1
  if (unreadResult(session)) return 2
  return processAlive(session) ? 3 : 4
}
export function backgroundSummary(activity?: ActivityState): { label: string; detail: string; showJobs: boolean } {
  const coverage = activity?.background.coverage
  if (!activity) return { label: t("Фоновые задачи: статус неизвестен"), detail: t("Сведения об интеграции и фоновых задачах недоступны. Отсутствие списка не означает, что задач нет."), showJobs: false }
  if (coverage === 'unsupported') return { label: t("Нет интеграции фоновых задач"), detail: t("Этот адаптер не передаёт сведения о фоновых задачах. Отсутствие списка не означает, что задач нет."), showJobs: false }
  if (coverage === 'awaiting_snapshot') return { label: t("Ожидается снимок фоновых задач"), detail: t("Интеграция доступна, но нативный снимок ещё не получен."), showJobs: activity.background.jobs.length > 0 }
  if (coverage === 'unknown') return { label: t("Фоновые задачи: статус неизвестен"), detail: t("Достоверный снимок сейчас недоступен. Это не подтверждает остановку или успешную работу задач."), showJobs: activity.background.jobs.length > 0 }
  return { label: t("Нативный снимок фоновых задач"), detail: t("Показано состояние на момент снимка. Время наблюдения не является подтверждением успешного выполнения или heartbeat."), showJobs: true }
}
/** Tool names come from authenticated native events. Arguments and output are
 * never needed here. A PreToolUse observation is a call, not proof it ran. */
export function toolCallLabel(session: ActivitySession): string {
  const tools = session.activity?.tools
  if (!processAlive(session) || session.activity?.tools_observed_at == null || !tools?.length) return ''
  const names = [...new Set(tools.map(tool => tool.name))]
  const display = names.slice(0, 2).join(', ') + (names.length > 2 ? t(' и ещё {count}', { count: names.length - 2 }) : '')
  return t('Вызов инструмента: {name}', { name: display })
}
export function backgroundInventoryLabel(session: ActivitySession): string {
  const background = session.activity?.background
  if (!background || background.coverage === 'unsupported' || !background.jobs.length) return ''
  const scheduled = background.jobs.filter(job => job.kind === 'cron').length
  const failed = background.jobs.filter(job => job.status === 'failed').length
  const label = t('Фон: {count}', { count: background.jobs.length }) + (scheduled ? t(' · по расписанию: {count}', { count: scheduled }) : '') + (failed ? t(' · ошибок: {count}', { count: failed }) : '')
  return background.coverage === 'snapshot' ? t('{label} · снимок', { label }) : t('{label} · неполные данные', { label })
}
export function activityTime(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return t("неизвестно")
  const date = new Date(value < 1e12 ? value * 1000 : value)
  return Number.isNaN(date.getTime()) ? t("неизвестно") : date.toLocaleString(localeTag())
}
