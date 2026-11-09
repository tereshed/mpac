import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage, isRunning } from './api'
import type { Project, Root, Session } from './api'
import type { LaunchForReview, ReviewBatch, ReviewCatalog, ReviewComment } from './review-types'
import { commentDelivery, commentRangeLabel, utf8Length } from './review-selection'
import ReviewBatchDialog from './ReviewBatchDialog'
import Modal from './Modal'
import './review-comments.css'

interface Props { token: string; project: Project; root: Root; sessions: Session[]; revision?: number; onLaunchForReview?: LaunchForReview; onOpenSession?: (session: Session) => void }
export default function ReviewCommentsPanel(props: Props) {
  return <CommentsPanel key={props.token + '\0' + props.project.id + '\0' + props.root.id} {...props} />
}
function CommentsPanel({ token, project, root, sessions, revision = 0, onLaunchForReview, onOpenSession }: Props) {
  const [catalog, setCatalog] = useState<ReviewCatalog>({ comments: [], batches: [] })
  const [error, setError] = useState('')
  const [refresh, setRefresh] = useState(0)
  const [allRoots, setAllRoots] = useState(false)
  const [showAll, setShowAll] = useState(false)
  const [targetId, setTargetId] = useState('')
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [batchOpen, setBatchOpen] = useState(false)
  const [existingBatch, setExistingBatch] = useState<ReviewBatch | null>(null)
  const [editing, setEditing] = useState<ReviewComment | null>(null)
  const [deleting, setDeleting] = useState<ReviewComment | null>(null)
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const context = token + '\0' + project.id
  const latest = useRef(context); latest.current = context
  const targetRef = useRef(targetId); targetRef.current = targetId
  const seen = useRef(new Map<string, number>())
  const pending = useRef(false)
  const mutation = useRef<AbortController | null>(null)
  const activeSessions = sessions.filter(session => session.project_id === project.id && isRunning(session))
  const target = sessions.find(session => session.id === targetId)
  const visibleComments = catalog.comments.filter(comment => !comment.deleted && (allRoots || comment.root_id === root.id))
  const shown = visibleComments.filter(comment => showAll || !targetId || commentDelivery(comment, targetId, catalog.batches) !== 'handed_off')
  const picked = visibleComments.filter(comment => selected.has(comment.id) && (showAll || !targetId || commentDelivery(comment, targetId, catalog.batches) !== 'handed_off'))
  const batches = catalog.batches.filter(batch => ['prepared', 'preparing', 'uncertain'].includes(batch.status)).slice(0, 5)
  useEffect(() => { const timer = setInterval(() => setRefresh(value => value + 1), 7000); return () => clearInterval(timer) }, [])
  useEffect(() => {
    const abort = new AbortController()
    api<ReviewCatalog>(token, `/projects/${project.id}/comments`, undefined, abort.signal).then(result => {
      if (abort.signal.aborted || latest.current !== context) return
      setCatalog(result); setError('')
      setSelected(previous => {
        const next = new Set(previous)
        for (const comment of result.comments) {
          if (comment.deleted) { next.delete(comment.id); continue }
          if (seen.current.get(comment.id) !== comment.revision && (!targetRef.current || commentDelivery(comment, targetRef.current, result.batches) !== 'handed_off')) next.add(comment.id)
          seen.current.set(comment.id, comment.revision)
        }
        return next
      })
    }).catch(failure => { if (!abort.signal.aborted && latest.current === context) setError(errorMessage(failure)) })
    return () => abort.abort()
  }, [token, project.id, refresh, revision])
  useEffect(() => {
    if (targetId) return
    const compatible = activeSessions.find(session => session.root_id === root.id || session.additional_roots?.some(extra => extra.id === root.id))
    if (compatible) setTargetId(compatible.id)
  }, [sessions, root.id, targetId])
  useEffect(() => () => { mutation.current?.abort() }, [])
  const changeTarget = (id: string) => {
    setTargetId(id)
    setSelected(previous => new Set([...previous].filter(commentId => { const comment = catalog.comments.find(item => item.id === commentId); return comment && !comment.deleted && (showAll || commentDelivery(comment, id, catalog.batches) !== 'handed_off') })))
  }
  const saveComment = async (remove: boolean) => {
    const comment = remove ? deleting : editing
    if (!comment || pending.current || (!remove && (!draft.trim() || utf8Length(draft.trim()) > 4000))) return
    pending.current = true; setBusy(true); setError('')
    const abort = new AbortController(); mutation.current = abort
    try {
      await api(token, `/review-comments/${comment.id}/${remove ? 'delete' : 'update'}`, { expected_revision: comment.revision, ...(!remove ? { text: draft.trim() } : {}) }, abort.signal)
      if (!abort.signal.aborted && latest.current === context) { setEditing(null); setDeleting(null); setRefresh(value => value + 1) }
    } catch (failure) { if (!abort.signal.aborted && latest.current === context) setError(errorMessage(failure)) }
    finally { pending.current = false; if (!abort.signal.aborted) setBusy(false); if (mutation.current === abort) mutation.current = null }
  }
  const deliveryLabel = (comment: ReviewComment) => {
    const status = targetId ? commentDelivery(comment, targetId, catalog.batches) : 'new'
    return status === 'handed_off' ? t("Передано CLI · подтверждено вами") : status === 'prepared' ? t("В черновике CLI") : status === 'uncertain' ? t("Доставка требует проверки") : t("Не отправлен")
  }
  const revalidate = async (comment: ReviewComment) => {
    if (pending.current) return
    const abort = new AbortController(); mutation.current = abort; pending.current = true; setBusy(true); setError('')
    try {
      await api(token, `/review-comments/${comment.id}/revalidate`, { expected_revision: comment.revision }, abort.signal)
      if (!abort.signal.aborted && latest.current === context) setRefresh(value => value + 1)
    } catch (failure) { if (!abort.signal.aborted && latest.current === context) setError(errorMessage(failure)) }
    finally { pending.current = false; if (mutation.current === abort) mutation.current = null; if (!abort.signal.aborted) setBusy(false) }
  }
  return <aside className="review-comments-panel" aria-label={t("Комментарии к ревью")}>
    <div className="review-comments-heading"><h3>{t("Комментарии")}{' '}<span>{visibleComments.length}</span></h3><button type="button" aria-label={t("Обновить комментарии")} onClick={() => setRefresh(value => value + 1)}>↻</button></div>
    <label className="comments-target-label">{t("Адресат")}<select value={targetId} onChange={event => changeTarget(event.target.value)}><option value="">{t("Выбрать сессию или создать новую…")}</option>{activeSessions.map(session => <option key={session.id} value={session.id}>{session.title}</option>)}{target && !isRunning(target) && <option value={target.id}>{target.title}{' '}{t("· завершена")}</option>}</select></label>
    {project.roots.length > 1 && <label className="comments-toggle"><input type="checkbox" checked={allRoots} onChange={event => setAllRoots(event.target.checked)} />{t("Весь проект")}</label>}
    <div className="comments-selection-tools"><button type="button" aria-pressed={!showAll} onClick={() => { setShowAll(false); setSelected(previous => new Set([...previous].filter(id => { const comment = catalog.comments.find(item => item.id === id); return comment && (!targetId || commentDelivery(comment, targetId, catalog.batches) !== 'handed_off') }))) }}>{t("Не отправлены")}</button><button type="button" aria-pressed={showAll} onClick={() => setShowAll(true)}>{t("Все")}</button></div>
    <div className="comments-selection-tools"><button type="button" onClick={() => setSelected(previous => new Set([...previous, ...shown.map(comment => comment.id)]))}>{t("Выбрать все")}</button><button type="button" onClick={() => setSelected(new Set())}>{t("Снять выбор")}</button></div>
    {shown.map(comment => <article className="review-comment-card" key={comment.id}><label><input type="checkbox" checked={selected.has(comment.id)} onChange={event => setSelected(previous => { const next = new Set(previous); event.target.checked ? next.add(comment.id) : next.delete(comment.id); return next })} /><span><strong>{comment.path}</strong><small>{allRoots && <>{project.roots.find(item => item.id === comment.root_id)?.name || t("Папка удалена")} · </>}{commentRangeLabel(comment)}</small></span></label><p>{comment.text}</p><div className="comment-delivery">{deliveryLabel(comment)} · v{comment.revision}</div>{comment.stale && <p className="comment-stale">{t("Код изменился · привязку нужно проверить")}</p>}<div className="comment-card-actions">{comment.stale && <button type="button" disabled={busy} onClick={() => void revalidate(comment)}>{t("Проверить привязку")}</button>}<button type="button" disabled={busy} onClick={() => { setEditing(comment); setDraft(comment.text); setError('') }}>{t("Изменить")}</button><button type="button" disabled={busy} onClick={() => { setDeleting(comment); setError('') }}>{t("Удалить…")}</button></div></article>)}
    {!shown.length && <p className="comments-empty">{visibleComments.length ? t("Для этого адресата новых комментариев нет.") : t("Выделите строки кода или оставьте комментарий к файлу.")}</p>}
    {error && !editing && !deleting && <div className="inline-error" role="alert">{t(error)}</div>}
    <button type="button" className="button primary comments-prepare" onClick={() => { setExistingBatch(null); setBatchOpen(true) }}>{t("Подготовить пакет ·")}{' '}{picked.length}</button>
    {picked.length > 32 && <p className="comment-stale">{t("В одном пакете может быть не больше 32 комментариев.")}</p>}
    {!!batches.length && <div className="review-batches"><h4>{t("Подготовленные пакеты")}</h4>{batches.map(batch => <button type="button" key={batch.id} onClick={() => { setExistingBatch(batch); setBatchOpen(true) }}><span>{sessions.find(session => session.id === batch.session_id)?.title || t("Сессия")} · {batch.comments.length}</span><small>{batch.status === 'prepared' ? t("Черновик вставлен") : batch.status === 'preparing' ? t("Вставка не подтверждена") : t("Доставка неизвестна")}</small></button>)}</div>}
    {batchOpen && <ReviewBatchDialog token={token} project={project} root={root} comments={visibleComments} initialSelectedIds={picked.map(comment => comment.id)} sessions={sessions} initialTarget={targetId} existingBatch={existingBatch} onClose={() => { setBatchOpen(false); setExistingBatch(null); setRefresh(value => value + 1) }} onTarget={changeTarget} onLaunchForReview={onLaunchForReview} onOpenSession={onOpenSession} onChanged={() => setRefresh(value => value + 1)} />}
    {(editing || deleting) && <Modal title={deleting ? t("Удалить комментарий?") : t("Изменить комментарий")} subtitle={(deleting || editing)?.path || ''} busy={busy} onClose={() => { if (!pending.current) { setEditing(null); setDeleting(null) } }}><p className="comments-dialog-help">{deleting ? t("Комментарий будет скрыт из ревью. Состав уже подготовленных пакетов останется в истории.") : t("Изменение создаёт новую версию комментария. Отправленная ранее версия остаётся в истории.")}</p>{editing && <textarea aria-label={t("Текст комментария")} rows={5} value={draft} onChange={event => setDraft(event.target.value)} disabled={busy} />}{deleting && <blockquote>{deleting.text}</blockquote>}{error && <div className="inline-error" role="alert">{t(error)}</div>}<div className="modal-actions"><button type="button" className="button secondary" disabled={busy} onClick={() => { setEditing(null); setDeleting(null) }}>{t("Отмена")}</button><button type="button" className={deleting ? 'button danger' : 'button primary'} disabled={busy || (!!editing && (!draft.trim() || utf8Length(draft.trim()) > 4000))} onClick={() => void saveComment(!!deleting)}>{busy ? t("Сохраняем…") : deleting ? t("Удалить комментарий") : t("Сохранить")}</button></div></Modal>}
  </aside>
}
