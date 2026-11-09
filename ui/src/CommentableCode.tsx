import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from './api'
import type { ReviewAnchor, ReviewComment, ReviewLine, ReviewSide } from './review-types'
import { codeRangeLabel, selectedCodeLines, selectionAnchor, utf8Length } from './review-selection'
import type { CodeSelection } from './review-selection'
import { keepReviewDraft, readReviewDraft } from './review-drafts'
import './review-comments.css'

interface Props {
  token: string; projectId: string; rootId: string; path: string; source: 'diff' | 'file'; snapshot: string;
  blockId?: number; lines: ReviewLine[]; disabled?: boolean; focusLine?: number; focusEndLine?: number; onCreated: (comment: ReviewComment) => void;
}
interface Editor { anchor: ReviewAnchor; label: string; excerpt: string; excerptLines?: ReviewLine[]; context: string; snapshot: string }
export default function CommentableCode({ token, projectId, rootId, path, source, snapshot, blockId, lines, disabled = false, focusLine, focusEndLine, onCreated }: Props) {
  const context = token + '\0' + projectId + '\0' + rootId + '\0' + path + '\0' + source + '\0' + (blockId ?? '')
  const [selection, setSelection] = useState<CodeSelection | null>(null)
  const selectionRef = useRef(selection)
  const [editor, setEditor] = useState<Editor | null>(() => readReviewDraft<Editor>(context).editor)
  const [text, setText] = useState(() => readReviewDraft<Editor>(context).text)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const container = useRef<HTMLDivElement>(null)
  const drag = useRef<{ first: number; side: ReviewSide } | null>(null)
  const request = useRef<AbortController | null>(null)
  const pending = useRef(false)
  const currentDraft = useRef({ editor, text }); currentDraft.current = { editor, text }
  const latest = useRef(context)
  latest.current = context
  const choose = (next: CodeSelection | null) => { selectionRef.current = next; setSelection(next) }
  useEffect(() => { choose(null); drag.current = null }, [context, snapshot])
  useEffect(() => {
    if (source === 'file' && focusLine) container.current?.querySelector<HTMLElement>(`[data-source-line="${focusLine}"]`)?.scrollIntoView({ block: 'center' })
  }, [source, focusLine, snapshot, context])
  useEffect(() => () => { request.current?.abort() }, [])
  useEffect(() => () => { keepReviewDraft(context, currentDraft.current.editor, currentDraft.current.text) }, [context])
  useEffect(() => {
    const end = () => {
      if (drag.current) { drag.current = null; window.getSelection()?.removeAllRanges(); return }
      const native = window.getSelection()
      if (!native || native.isCollapsed || !native.toString().trim()) return
      const element = (node: Node | null) => node instanceof Element ? node : node?.parentElement
      const first = element(native.anchorNode)?.closest<HTMLElement>('[data-review-row]')
      const last = element(native.focusNode)?.closest<HTMLElement>('[data-review-row]')
      if (!first || !last || !container.current?.contains(first) || !container.current.contains(last)) return
      choose({ first: Number(first.dataset.reviewRow), last: Number(last.dataset.reviewRow), side: 'both' }); native.removeAllRanges()
    }
    const cancel = () => { drag.current = null }
    document.addEventListener('pointerup', end); document.addEventListener('pointercancel', cancel)
    return () => { document.removeEventListener('pointerup', end); document.removeEventListener('pointercancel', cancel) }
  }, [])
  const rows = selectedCodeLines(lines, selection)
  const selected = new Set(rows.map(line => line.index))
  const beginEditor = (anchor: ReviewAnchor, label: string, chosen: ReviewLine[]) => {
    if (disabled || pending.current) return
    setError(''); setEditor({ anchor, label, excerpt: '', excerptLines: chosen.map(line => ({ ...line })), snapshot, context })
  }
  const commentSelection = (value = selectionRef.current) => {
    if (!value) return
    try { beginEditor(selectionAnchor(lines, value, blockId), codeRangeLabel(lines, value), selectedCodeLines(lines, value)) }
    catch (failure) { setError(errorMessage(failure)) }
  }
  const submit = async () => {
    if (!editor || !text.trim() || pending.current || disabled || editor.context !== context || editor.snapshot !== snapshot || utf8Length(text.trim()) > 4000) return
    const abort = new AbortController(); request.current = abort; pending.current = true; setBusy(true); setError('')
    try {
      const saved = await api<ReviewComment>(token, `/projects/${projectId}/comments`, { root_id: rootId, path, source, snapshot: editor.snapshot, text: text.trim(), anchor: editor.anchor }, abort.signal)
      if (!abort.signal.aborted && latest.current === context) { setText(''); setEditor(null); onCreated(saved) }
    } catch (failure) { if (!abort.signal.aborted && latest.current === context) setError(errorMessage(failure)) }
    finally { pending.current = false; if (!abort.signal.aborted) setBusy(false); if (request.current === abort) request.current = null }
  }
  const editorLabel = !editor ? '' : editor.anchor.kind === 'file' ? t("Весь файл")
    : editor.excerptLines ? codeRangeLabel(editor.excerptLines, { first: editor.anchor.start_row, last: editor.anchor.end_row, side: editor.anchor.side })
    : editor.label.split(' · ').map(part => t(part)).join(' · ')
  const editorExcerpt = editor?.excerptLines?.map(line => t("{p0} [до: {p1}; после: {p2}] {p3}", { p0: line.kind === 'addition' ? '+' : line.kind === 'deletion' ? '−' : ' ', p1: line.old_line ?? '—', p2: line.new_line ?? '—', p3: line.text })).join('\n') ?? editor?.excerpt
  const staleEditor = editor && (editor.snapshot !== snapshot || editor.context !== context)
  return <div className="commentable-code" ref={container}>
    <div className="range-help"><span>{t("Мышью по коду или номерам строк · Shift + клик — диапазон")}</span><button type="button" disabled={disabled || busy} onClick={() => beginEditor({ kind: 'file' }, t("Весь файл"), [])}>{t("Комментарий к файлу")}</button></div>
    <div className={source === 'file' ? 'range-code source-lines' : 'range-code'} onPointerDown={event => {
      if (event.button !== 0 || disabled || busy) return
      const target = event.target as Element
      const number = target.closest<HTMLElement>('[data-select-side]')
      const row = target.closest<HTMLElement>('[data-review-row]')
      if (!row || (!number && !target.closest('code'))) return
      const side: ReviewSide = number ? number.dataset.selectSide as ReviewSide : 'both'
      if (number) event.preventDefault()
      const index = Number(row.dataset.reviewRow), previous = selectionRef.current
      const first = event.shiftKey && previous && previous.side === side ? previous.first : index
      drag.current = { first, side }; choose({ first, last: index, side })
    }} onPointerOver={event => {
      const row = (event.target as Element).closest<HTMLElement>('[data-review-row]')
      if (drag.current && row) choose({ ...drag.current, last: Number(row.dataset.reviewRow) })
    }}>
      {lines.map((line, index) => <div className="range-row-group" key={line.index}>
        {source === 'diff' && (index === 0 || lines[index - 1].hunk !== line.hunk) && <div className="range-hunk"><span>{t("Блок изменений")}{' '}{line.hunk + 1}</span><button type="button" disabled={disabled || busy} onClick={() => { const hunk = lines.filter(item => item.hunk === line.hunk); choose({ first: hunk[0].index, last: hunk[hunk.length - 1].index, side: 'both' }) }}>{t("Выделить блок")}</button></div>}
        <div className={`range-code-row ${line.kind} ${selected.has(line.index) ? 'range-selected' : ''} ${source === 'file' && focusLine && line.new_line !== null && line.new_line >= focusLine && line.new_line <= (focusEndLine || focusLine) ? 'file-linked-line' : ''}`} data-review-row={line.index} data-source-line={source === 'file' ? line.new_line ?? undefined : undefined}>
          {([['old', line.old_line], ['new', line.new_line]] as const).map(([side, number]) => number === null ? <span key={side} /> : <button type="button" className="range-line-number" key={side} data-select-side={side} aria-label={t("Выбрать {p0} строку {p1}", { p0: side === 'old' ? t("старую") : t("новую"), p1: number })} disabled={disabled || busy} onClick={event => { if (event.detail === 0) { const previous = selectionRef.current; choose({ first: event.shiftKey && previous?.side === side ? previous.first : line.index, last: line.index, side }) } }}>{number}</button>)}
          <code>{source === 'diff' ? line.kind === 'addition' ? '+ ' : line.kind === 'deletion' ? '− ' : '  ' : ''}{line.text || ' '}</code>
          <button type="button" className="range-line-comment" aria-label={t("Комментарий к {p0} строке {p1}", { p0: line.kind === 'deletion' ? t("старой") : t("новой"), p1: line.kind === 'deletion' ? line.old_line : line.new_line })} disabled={disabled || busy} onClick={() => { let value = selectionRef.current; if (!value || !selectedCodeLines(lines, value).some(item => item.index === line.index)) { value = { first: line.index, last: line.index, side: line.kind === 'deletion' ? 'old' : 'new' }; choose(value) } commentSelection(value) }}>+</button>
        </div>
      </div>)}
    </div>
    {selection && rows.length > 0 && <div className="range-selection-summary"><span><strong>{t("Выделено строк:")}{' '}{rows.length}</strong><small>{codeRangeLabel(lines, selection)}</small></span><button type="button" className="button primary compact" disabled={disabled || busy} onClick={() => commentSelection()}>{t("Комментарий к выделению")}</button><button type="button" disabled={busy} onClick={() => choose(null)}>{t("Снять выделение")}</button></div>}
    {editor && <div className="range-editor"><strong>{editorLabel}</strong><p>{editor.anchor.kind === 'file' ? t("Замечание относится ко всему файлу.") : t("Замечание относится ко всему выделенному диапазону строк.")}</p>{editorExcerpt && <pre>{editorExcerpt}</pre>}<label>{t("Комментарий")}<textarea value={text} onChange={event => setText(event.target.value)} disabled={busy} rows={3} placeholder={t("Что изменить в выделенном фрагменте?")} /></label>{utf8Length(text.trim()) > 4000 && <div className="inline-error">{t("Комментарий должен занимать не больше 4000 байт UTF-8.")}</div>}{staleEditor && <div className="notice">{t("Код или рабочая папка изменились. Текст сохранён в поле; выберите актуальный фрагмент для привязки.")}</div>}<div className="range-editor-actions"><button type="button" className="button secondary compact" disabled={busy} onClick={() => { setEditor(null); setText('') }}>{t("Отмена")}</button><button type="button" className="button primary compact" disabled={disabled || busy || !text.trim() || !!staleEditor || utf8Length(text.trim()) > 4000} onClick={() => void submit()}>{busy ? t("Сохраняем…") : t("Сохранить комментарий")}</button></div></div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
  </div>
}
