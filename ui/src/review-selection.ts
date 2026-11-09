import { t } from './i18n.ts'
import type { ReviewAnchor, ReviewComment, ReviewLine, ReviewSide, ReviewBatch } from './review-types'
import type { Session } from './api'

export interface CodeSelection { first: number; last: number; side: ReviewSide }
export function selectedCodeLines(lines: ReviewLine[], selection: CodeSelection | null): ReviewLine[] {
  if (!selection) return []
  const first = Math.min(selection.first, selection.last), last = Math.max(selection.first, selection.last)
  return lines.filter(line => line.index >= first && line.index <= last && (selection.side === 'both' || (selection.side === 'old' ? line.old_line : line.new_line) !== null))
}
export function selectionAnchor(lines: ReviewLine[], selection: CodeSelection, blockId?: number): ReviewAnchor {
  const selected = selectedCodeLines(lines, selection)
  if (!selected.length) throw new Error(t("Выделите строки кода."))
  const span = lines.filter(line => line.index >= Math.min(selection.first, selection.last) && line.index <= Math.max(selection.first, selection.last))
  if (span.length > 200) throw new Error(t("Можно прокомментировать не больше 200 строк за один раз."))
  if (new Set(span.map(line => line.hunk)).size !== 1) throw new Error(t("Диапазон не должен пересекать пропущенные участки diff. Оставьте отдельный комментарий к каждому блоку."))
  return { kind: 'range', ...(blockId === undefined ? {} : { block_id: blockId }), start_row: Math.min(selection.first, selection.last), end_row: Math.max(selection.first, selection.last), side: selection.side }
}
function rangeLabel(start: number | null, end: number | null, old: boolean): string {
  if (start === null) return ''
  return start === end || end === null
    ? t(old ? "Старая строка {p0}" : "Новая строка {p0}", { p0: start })
    : t(old ? "Старые строки {p0}–{p1}" : "Новые строки {p0}–{p1}", { p0: start, p1: end })
}
export function codeRangeLabel(lines: ReviewLine[], selection: CodeSelection | null): string {
  if (!selection) return t("Выделите текст или номера строк")
  const selected = selectedCodeLines(lines, selection)
  const old = selection.side === 'new' ? [] : selected.flatMap(line => line.old_line === null ? [] : [line.old_line])
  const current = selection.side === 'old' ? [] : selected.flatMap(line => line.new_line === null ? [] : [line.new_line])
  return [rangeLabel(old.length ? Math.min(...old) : null, old.length ? Math.max(...old) : null, true), rangeLabel(current.length ? Math.min(...current) : null, current.length ? Math.max(...current) : null, false)].filter(Boolean).join(' · ')
}
export function commentRangeLabel(comment: ReviewComment): string {
  return comment.anchor.kind === 'file' ? t("Весь файл") : [rangeLabel(comment.old_start, comment.old_end, true), rangeLabel(comment.new_start, comment.new_end, false)].filter(Boolean).join(' · ')
}
export function commentDelivery(comment: ReviewComment, sessionId: string, batches: ReviewBatch[]): 'new' | 'prepared' | 'handed_off' | 'uncertain' {
  if (comment.deliveries?.some(delivery => delivery.session_id === sessionId && delivery.revision === comment.revision && delivery.status === 'handed_off')) return 'handed_off'
  const matching = batches.filter(batch => batch.session_id === sessionId && batch.comments.some(entry => entry.id === comment.id && entry.revision === comment.revision))
  if (matching.some(batch => batch.status === 'handed_off')) return 'handed_off'
  if (matching.some(batch => batch.status === 'uncertain' || batch.status === 'preparing')) return 'uncertain'
  return matching.some(batch => batch.status === 'prepared') ? 'prepared' : 'new'
}
export function batchCommentsForTarget(comments: ReviewComment[], sessionId: string, includeSent: boolean): ReviewComment[] {
  return comments.filter(comment => !comment.deleted && (includeSent || !sessionId || sessionId === '__new__' || commentDelivery(comment, sessionId, []) !== 'handed_off'))
}
export function initialBatchSelection(comments: ReviewComment[], selectedIds?: string[]): Set<string> {
  const existing = new Set(comments.filter(comment => !comment.deleted).map(comment => comment.id))
  return new Set(selectedIds === undefined ? existing : selectedIds.filter(id => existing.has(id)))
}
export function initialBatchShowAll(comments: ReviewComment[], selectedIds: string[] | undefined, sessionId: string): boolean {
  const selected = initialBatchSelection(comments, selectedIds)
  return comments.some(comment => selected.has(comment.id) && commentDelivery(comment, sessionId, []) === 'handed_off')
}
export function sessionCoversComments(session: Session, comments: ReviewComment[], projectId: string): boolean {
  if (session.project_id !== projectId || !['running', 'needs_attention'].includes(session.status)) return false
  const roots = new Set([session.root_id, ...(session.additional_roots || []).map(root => root.id)])
  return comments.every(comment => roots.has(comment.root_id))
}
export function utf8Length(value: string): number { return new TextEncoder().encode(value).length }
