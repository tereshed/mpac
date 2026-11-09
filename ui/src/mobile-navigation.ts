import { localeTag, t } from './i18n.ts'
import type { ActivityJob } from './activity-types'
import type { Session } from './api'
import { activitySummary, attentionPriority, backgroundSummary, backgroundInventoryLabel, failedBackgroundJobs, passiveBackground, unreadResult } from './activity.ts'

export type MobileSheet = 'attention' | 'sessions' | 'projects' | null
export type MobileSessionFilter = 'all' | 'input' | 'results' | 'working' | 'background' | 'history'
export type MobileSessionTone = 'attention' | 'error' | 'result' | 'working' | 'background' | 'idle' | 'history' | 'unknown'
/** Color reflects the same observed state as the text; an unread monitor
 * response remains passive and never becomes an action request. */
export function mobileSessionTone(session: Session): MobileSessionTone {
  if (attentionPriority(session) === 0) return 'attention'
  if (['failed', 'interrupted'].includes(session.status) || failedBackgroundJobs(session).length) return 'error'
  if (!['running', 'needs_attention'].includes(session.status)) return 'history'
  if (passiveBackground(session)) return 'background'
  const tone = activitySummary(session).tone
  return tone === 'attention' ? 'attention' : tone === 'stopped' ? 'history' : tone
}
export interface MobileAttentionReason { kind: 'input' | 'process' | 'background' | 'result'; label: string; detail: string; at: number | null }
export interface MobileAttentionEntry { session: Session; priority: number; reasons: MobileAttentionReason[] }
const countRules = new Map<string, Intl.PluralRules>()
export function mobileCount(value: number, forms: [string, string, string]): string {
  const locale = localeTag()
  let rules = countRules.get(locale)
  if (!rules) { rules = new Intl.PluralRules(locale); countRules.set(locale, rules) }
  const plural = rules.select(value)
  return `${new Intl.NumberFormat(locale).format(value)} ${t(forms[plural === 'one' ? 0 : plural === 'few' ? 1 : 2])}`
}

export function mobileTime(value: number | string | null | undefined): number | null {
  if (typeof value === 'string') { const time = Date.parse(value); return Number.isFinite(time) ? time : null }
  if (typeof value !== 'number' || !Number.isFinite(value)) return null
  const time = value < 1e12 ? value * 1000 : value
  return Number.isNaN(new Date(time).getTime()) ? null : time
}
export function mobileFailedJobs(session: Session): ActivityJob[] {
  return failedBackgroundJobs(session)
}
export function mobileHasBackground(session: Session): boolean {
  const activity = session.activity
  return !!activity && (activity.capabilities.background || activity.background.coverage === 'snapshot' || activity.background.coverage === 'awaiting_snapshot')
}
export function mobileBackgroundLabel(session: Session): string {
  const background = session.activity?.background
  if (background?.coverage === 'snapshot') {
    const failed = mobileFailedJobs(session).length
    const scheduled = background.jobs.filter(job => job.kind === 'cron').length
    return t("Снимок фона: {p0}{p1}", { p0: mobileCount(background.jobs.length, [t("задача"), t("задачи"), t("задач")]), p1: (scheduled ? t(' · по расписанию: {count}', { count: scheduled }) : '') + (failed ? t(" · ошибок: {p0}", { p0: failed }) : '') })
  }
  return backgroundInventoryLabel(session) || backgroundSummary(session.activity).label
}
export function mobileAttentionReasons(session: Session): MobileAttentionReason[] {
  const reasons: MobileAttentionReason[] = [], activity = session.activity
  if (attentionPriority(session) === 0) {
    const summary = activitySummary(session)
    reasons.push({ kind: 'input', label: summary.label, detail: summary.detail, at: mobileTime(activity?.attention?.since) })
  }
  if (['failed', 'interrupted'].includes(session.status)) reasons.push({ kind: 'process', label: session.status === 'failed' ? t("CLI завершился с ошибкой") : t("CLI был прерван"), detail: t("Откройте терминал, чтобы проверить причину."), at: mobileTime(session.last_activity) })
  const failed = mobileFailedJobs(session)
  if (failed.length) reasons.push({ kind: 'background', label: t("Ошибка фоновой задачи в снимке{p0}", { p0: failed.length > 1 ? ` · ${failed.length}` : '' }), detail: failed.slice(0, 3).map(job => job.name || job.id).join(', ') + t(". Состояние на момент наблюдения; не проверка текущей работоспособности."), at: mobileTime(activity?.background.observed_at) })
  // An unread response is informational. Keep it as context only when an
  // actual request/error already requires action; results have their own filter.
  if (reasons.length && unreadResult(session)) reasons.push({ kind: 'result', label: t("Новый результат"), detail: t("Откройте результат в терминале. Просмотр отмечается отдельной кнопкой."), at: mobileTime(activity?.result.at) })
  return reasons
}
function compareEntries(a: MobileAttentionEntry, b: MobileAttentionEntry): number {
  if (a.priority !== b.priority) return a.priority - b.priority
  const left = a.reasons[0]?.at, right = b.reasons[0]?.at
  if (left != null && right != null && left !== right) return a.priority === 0 ? left - right : right - left
  if (left != null && right == null) return -1
  if (right != null && left == null) return 1
  return a.session.id.localeCompare(b.session.id)
}
export function mobileAttentionEntries(sessions: Session[]): MobileAttentionEntry[] {
  return sessions.flatMap(session => {
    const reasons = mobileAttentionReasons(session)
    if (!reasons.length) return []
    const priority = Math.min(attentionPriority(session), mobileFailedJobs(session).length ? 1 : Infinity)
    return [{ session, reasons, priority }]
  }).sort(compareEntries)
}
export function mobileSessionMatches(session: Session, filter: MobileSessionFilter): boolean {
  if (filter === 'input') return attentionPriority(session) === 0
  if (filter === 'results') return unreadResult(session)
  if (filter === 'working') return ['running', 'needs_attention'].includes(session.status) && (session.agent === 'command' || session.activity?.phase === 'working')
  if (filter === 'background') return mobileHasBackground(session)
  if (filter === 'history') return !['running', 'needs_attention'].includes(session.status)
  return true
}
export function mobileSessions(sessions: Session[], projectId: string | null, filter: MobileSessionFilter): Session[] {
  const priorities = new Map(mobileAttentionEntries(sessions).map(entry => [entry.session.id, entry.priority]))
  return sessions.filter(session => (!projectId || session.project_id === projectId) && mobileSessionMatches(session, filter)).sort((a, b) => {
    const priority = (session: Session) => priorities.get(session.id) ?? attentionPriority(session)
    return priority(a) - priority(b) || (mobileTime(b.last_activity) ?? mobileTime(b.started_at) ?? 0) - (mobileTime(a.last_activity) ?? mobileTime(a.started_at) ?? 0) || a.id.localeCompare(b.id)
  })
}
