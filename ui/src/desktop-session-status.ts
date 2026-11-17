import type { Session } from './api'
import { activitySummary, attentionPriority, passiveBackground, unreadResult } from './activity.ts'
import { t } from './i18n.ts'

export const desktopSessionGroups = [
  { id: 'attention', label: 'Ожидают ввода', glyph: '◆' },
  { id: 'error', label: 'Ошибки и прерывания', glyph: '!' },
  { id: 'result', label: 'Новые результаты', glyph: '●' },
  { id: 'working', label: 'Работают', glyph: '▶' },
  { id: 'background', label: 'Фон', glyph: '◷' },
  { id: 'idle', label: 'Ожидают задачу', glyph: '·' },
  { id: 'unknown', label: 'CLI запущен · статус агента недоступен', glyph: '◉' },
  { id: 'history', label: 'История', glyph: '■' },
] as const

export function desktopSessionStatus(session: Session) {
  const priority = attentionPriority(session)
  if (priority === 0) return { id: 'attention', label: activitySummary(session).label }
  if (priority === 1) return { id: 'error', label: t('Нужно внимание') }
  if (passiveBackground(session)) return { id: 'background', label: t('Фон: {count}', { count: session.activity?.background.jobs.length ?? 0 }) }
  if (session.status === 'running' || session.status === 'needs_attention') {
    const activity = activitySummary(session)
    if (activity.tone === 'working') return { id: 'working', label: t('Работает') }
    if (activity.tone === 'idle' && !unreadResult(session)) return { id: 'idle', label: session.agent === 'shell' ? activity.label : t('Ожидает задачу') }
    if (activity.tone === 'unknown') return { id: 'unknown', label: t('CLI запущен') }
  }
  if (unreadResult(session)) return { id: 'result', label: t('Новый результат') }
  return { id: 'history', label: t('Завершена') }
}
