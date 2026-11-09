import { t } from './i18n.ts'
import type { Session } from './api'
import { attentionPriority } from './activity.ts'

export type SessionView = 'list' | 'dots'
export function sessionTitle(value: string): string {
  const title = value.trim()
  if (!title) throw new Error(t("Введите название сессии."))
  if (/[\u0000-\u001f\u007f-\u009f\u2028-\u202e\u2066-\u2069]/.test(title)) throw new Error(t("В названии не должно быть переносов строк или управляющих символов."))
  if (new TextEncoder().encode(title).byteLength > 120) throw new Error(t("Название слишком длинное: максимум 120 байт UTF-8."))
  return title
}
export function needsAttention(session: Session): boolean {
  return attentionPriority(session) < 2
}
export function orderedSessions(sessions: Session[], attentionOnly = false): Session[] {
  const timestamp = (s: Session) => typeof s.started_at === 'number' ? s.started_at < 1e12 ? s.started_at * 1000 : s.started_at : Date.parse(s.started_at) || 0
  return sessions.filter(s => !attentionOnly || needsAttention(s)).sort((a, b) => attentionPriority(a) - attentionPriority(b) || timestamp(b) - timestamp(a) || a.id.localeCompare(b.id))
}
export function nextSessionAfterRemoval(sessions: Session[], projectId: string, removedId: string): Session | undefined {
  return orderedSessions(sessions.filter(s => s.project_id === projectId && s.id !== removedId))[0]
}
export function readPreference(key: string, fallback: string): string {
  try { return localStorage.getItem(key) || fallback } catch { return fallback }
}
export function savePreference(key: string, value: string): void {
  try { localStorage.setItem(key, value) } catch { /* Preferences must not prevent local use. */ }
}
