import { t } from './i18n.ts'
import { useEffect, useMemo, useState } from 'react'
import { api, errorMessage, isRunning, query } from './api'
import type { Project, Review, Root, Session } from './api'
import { RootSelect } from './FilesPane'
import { reviewFileDiffs } from './reviewDiff'

export default function LegacyReviewPane({ token, project, root, session, onRoot, onSent, externalToolbar = false }: { token: string; project: Project; root: Root; session?: Session; onRoot: (id: string) => void; onSent: () => void; externalToolbar?: boolean }) {
  const [review, setReview] = useState<Review | null>(null)
  const [comments, setComments] = useState('')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [sending, setSending] = useState(false)
  const [revision, setRevision] = useState(0)
  const [sent, setSent] = useState(false)
  const [selectedFile, setSelectedFile] = useState<string | null>(null)

  useEffect(() => {
    const abort = new AbortController()
    setLoading(true); setReview(null); setSent(false); setError('')
    api<Review>(token, `/projects/${project.id}/review?${query({ root_id: root.id })}`, undefined, abort.signal)
      .then(value => {
        if (!abort.signal.aborted) {
          setReview(value)
          setSelectedFile(previous => value.files.some(file => file.path === previous) ? previous : null)
        }
      })
      .catch(e => { if (!abort.signal.aborted) setError(errorMessage(e)) })
      .finally(() => { if (!abort.signal.aborted) setLoading(false) })
    return () => abort.abort()
  }, [token, project.id, root.id, revision])

  useEffect(() => { setComments(''); setSent(false); setSelectedFile(null) }, [root.id, project.id])

  const fileDiffs = useMemo(() => review ? reviewFileDiffs(review.diff, review.files) : new Map<string, string>(), [review])
  const selected = review?.files.find(file => file.path === selectedFile)
  const visibleDiff = review ? selected ? fileDiffs.get(selected.path) || '' : review.diff : ''

  const compatible = session && session.project_id === project.id && (session.root_id === root.id || session.additional_roots?.some(extra => extra.id === root.id)) && isRunning(session)
  const send = async () => {
    if (!session || !review || !comments.trim() || !compatible || sending) return
    setSending(true); setError(''); setSent(false)
    try {
      await api(token, `/sessions/${session.id}/review`, { root_id: root.id, snapshot: review.snapshot, comments: comments.trim() })
      setComments(''); setSent(true); onSent()
    } catch (e) { setError(errorMessage(e)) }
    finally { setSending(false) }
  }

  return <div className="review-layout">
    {!externalToolbar && <div className="panel-toolbar"><RootSelect project={project} root={root} onChange={onRoot} /><span className="path-hint" title={root.path}>{root.path}</span><button className="button secondary compact" onClick={() => setRevision(v => v + 1)} disabled={loading}>{t("↻ Обновить")}</button></div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    {sent && <div className="success-notice">{t("Черновик вставлен в терминал. Проверьте текст и нажмите Enter.")}</div>}
    <div className="review-summary"><span className="eyebrow">{t("РАБОЧИЕ ИЗМЕНЕНИЯ")}</span><span>{review ? t("{p0} файлов", { p0: review.files.length }) : '…'}</span><span className="mono muted">{review?.head ? review.head.slice(0, 10) : t("без коммита")}</span></div>
    {review?.truncated && <div className="notice">{t("Снимок изменений превышает лимит; отправка замечаний для него недоступна. Разделите изменения на меньшие ревью или уменьшите объём изменений репозитория, затем обновите. Чтение файлов отдельно не снимает ограничение снимка.")}</div>}
    <div className="review-body">
      {!!review?.files.length && <nav className="review-file-list" aria-label={t("Изменённые файлы")}>
        <button type="button" className={`review-file-button review-all-files ${!selected ? 'selected' : ''}`} aria-pressed={!selected} onClick={() => setSelectedFile(null)}><span className="review-file-status">≡</span><span>{t("Все изменения")}</span><span className="review-file-count">{review.files.length}</span></button>
        {review.files.map(file => <button type="button" key={file.path} title={file.path} className={`review-file-button ${selected?.path === file.path ? 'selected' : ''}`} aria-pressed={selected?.path === file.path} onClick={() => setSelectedFile(file.path)}><span className={`review-file-status ${file.status.includes('D') ? 'deleted' : file.status.includes('A') || file.status === '??' ? 'added' : ''}`} title={`Git: ${file.status}`}>{file.status.trim() || 'M'}</span><span className="review-file-path">{file.path}</span></button>)}
      </nav>}
      <div className="review-diff-panel">
        {!!review?.files.length && <div className="review-diff-title"><span title={selected?.path}>{selected?.path || t("Все изменения")}</span>{selected && <span className="review-diff-status">{selected.status === '??' ? t("Новый файл") : selected.status.includes('D') ? t("Удалён") : selected.status.includes('R') ? t("Переименован") : selected.status.includes('A') ? t("Добавлен") : t("Изменён")}</span>}</div>}
        <div className="diff-scroll" key={selected?.path || 'all'}>
          {loading ? <div className="empty-state"><p>{t("Собираем изменения…")}</p></div> : visibleDiff ? <div className="diff-view">{visibleDiff.split('\n').map((line, i) => <div className={`diff-line ${line.startsWith('+++') || line.startsWith('---') || line.startsWith('diff ') ? 'diff-file' : line.startsWith('@@') ? 'diff-hunk' : line.startsWith('+') ? 'diff-add' : line.startsWith('-') ? 'diff-remove' : ''}`} key={i}><span className="diff-line-number">{i + 1}</span><code>{line || ' '}</code></div>)}</div> : selected ? <div className="empty-state"><h3>{t("Текстовый diff недоступен")}</h3><p>{t("Изменение есть в списке, но его diff не вошёл в этот снимок. Проверьте общий просмотр и предупреждение об ограничении размера.")}</p></div> : <div className="empty-state"><div className="empty-symbol">✓</div><h3>{error ? t("Ревью недоступно") : t("Рабочая папка чистая")}</h3><p>{error ? t("Выберите Git-репозиторий или повторите запрос.") : t("Новые изменения появятся здесь после обновления.")}</p></div>}
        </div>
      </div>
    </div>
    <div className="review-compose"><label htmlFor="review-comments">{t("Замечания агенту")}</label><textarea id="review-comments" value={comments} onChange={e => { setComments(e.target.value); setSent(false) }} placeholder={t("Что нужно исправить? Укажите файл и ожидаемый результат…")} rows={3} maxLength={16000} /><div className="compose-footer"><span className="muted small">{compatible ? t("Черновик для «{p0}». После проверки нажмите Enter в терминале. Снимок проверяется при вставке.", { p0: session.title }) : t("Выберите работающую сессию в этой папке для отправки замечаний.")}</span><button className="button primary" disabled={!compatible || !comments.trim() || !review || sending || review.truncated} onClick={send}>{sending ? t("Вставляем…") : t("Вставить черновик ↗")}</button></div></div>
  </div>
}
