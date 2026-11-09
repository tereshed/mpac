import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import Modal from './Modal'
import { api, errorMessage, isRunning } from './api'
import type { Project, Root, Session } from './api'
import type { LaunchForReview, ReviewBatch, ReviewComment } from './review-types'
import { batchCommentsForTarget, commentDelivery, commentRangeLabel, initialBatchSelection, initialBatchShowAll, sessionCoversComments, utf8Length } from './review-selection'

interface Props {
  token: string; project: Project; root: Root; comments: ReviewComment[]; sessions: Session[]; initialTarget: string; existingBatch?: ReviewBatch | null;
  initialSelectedIds?: string[];
  onClose: () => void; onTarget: (id: string) => void; onChanged: () => void; onLaunchForReview?: LaunchForReview; onOpenSession?: (session: Session) => void;
}
export default function ReviewBatchDialog({ token, project, root, comments, sessions, initialSelectedIds, initialTarget, existingBatch, onClose, onTarget, onChanged, onLaunchForReview, onOpenSession }: Props) {
  const [candidates] = useState(comments)
  const [selected, setSelected] = useState(() => initialBatchSelection(comments, initialSelectedIds))
  const [targetId, setTargetId] = useState(existingBatch?.session_id || initialTarget)
  const [launched, setLaunched] = useState<Session | null>(null)
  const [message, setMessage] = useState(existingBatch?.message || '')
  const [showAll, setShowAll] = useState(() => initialBatchShowAll(comments, initialSelectedIds, existingBatch?.session_id || initialTarget))
  const [batch, setBatch] = useState<ReviewBatch | null>(existingBatch || null)
  const [confirmed, setConfirmed] = useState(false)
  const [busy, setBusy] = useState<'preview' | 'insert' | 'confirm' | null>(null)
  const [error, setError] = useState('')
  const mounted = useRef(true)
  const pending = useRef(false)
  const request = useRef<AbortController | null>(null)
  const ids = useRef(new Map<string, string>())
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; request.current?.abort() } }, [])
  const options = launched && !sessions.some(session => session.id === launched.id) ? [...sessions, launched] : sessions
  const projectSessions = options.filter(session => session.project_id === project.id)
  const target = projectSessions.find(session => session.id === targetId)
  const shown = batchCommentsForTarget(candidates, targetId, showAll)
  const picked = shown.filter(comment => selected.has(comment.id))
  const compatible = !!target && sessionCoversComments(target, picked, project.id)
  const valid = compatible && picked.length <= 32 && (picked.length > 0 || !!message.trim()) && utf8Length(message.trim()) <= 4000 && !picked.some(comment => comment.stale || comment.deleted)
  const changeTarget = (id: string) => {
    setTargetId(id); setError(''); setConfirmed(false)
    if (id !== '__new__') setSelected(previous => new Set([...previous].filter(commentId => { const comment = candidates.find(item => item.id === commentId); return comment && (showAll || commentDelivery(comment, id, []) !== 'handed_off') })))
  }
  const run = async (action: 'preview' | 'insert' | 'confirm') => {
    if (pending.current || (action === 'preview' && !valid) || (action === 'insert' && (!batch || batch.status !== 'draft' || !target || !isRunning(target))) || (action === 'confirm' && (!batch || !confirmed || !['prepared', 'uncertain'].includes(batch.status)))) return
    const abort = new AbortController(); request.current = abort; pending.current = true; setBusy(action); setError('')
    try {
      let result: ReviewBatch
      if (action === 'preview') {
        const payload = { session_id: targetId, comments: picked.map(comment => ({ id: comment.id, revision: comment.revision })), message: message.trim() }
        const key = JSON.stringify(payload)
        let id = ids.current.get(key)
        if (!id) { id = crypto.randomUUID(); ids.current.set(key, id) }
        result = await api<ReviewBatch>(token, `/projects/${project.id}/review-batches`, { ...payload, request_id: id }, abort.signal)
      } else {
        result = await api<ReviewBatch>(token, `/review-batches/${batch!.id}/${action === 'insert' ? 'insert' : 'confirm'}`, action === 'confirm' ? { confirmed: true } : {}, abort.signal)
      }
      if (!mounted.current || abort.signal.aborted) return
      setBatch(result); onTarget(result.session_id); onChanged()
    } catch (failure) {
      if (mounted.current && !abort.signal.aborted) { setError(errorMessage(failure)); onChanged() }
    } finally { pending.current = false; if (request.current === abort) request.current = null; if (mounted.current) setBusy(null) }
  }
  const launchNew = () => {
    if (!onLaunchForReview || pending.current) return
    const roots = [...new Set(picked.map(comment => comment.root_id))]
    onLaunchForReview({ rootId: roots.includes(root.id) || !roots.length ? root.id : roots[0], requiredRootIds: roots, onReady: session => {
      if (!mounted.current) return
      setLaunched(session); setTargetId(session.id); onTarget(session.id); setError('')
    } })
  }
  const status = batch?.status
  return <Modal title={batch ? t("Пакет ревью") : t("Передать ревью агенту")} subtitle={project.name + t(" · один пакет с задачей и комментариями")} busy={!!busy} onClose={() => { if (!pending.current) onClose() }}>
    {!batch ? <>
      <label>{t("Кому")}<select value={targetId} disabled={!!busy} onChange={event => changeTarget(event.target.value)}><option value="">{t("Выберите сессию…")}</option>{projectSessions.map(session => <option key={session.id} value={session.id} disabled={!isRunning(session)}>{session.title}{!isRunning(session) ? t(" · завершена") : ''}</option>)}{onLaunchForReview && <option value="__new__">{t("Новая сессия…")}</option>}</select></label>
      {targetId === '__new__' && <div className="batch-new-session"><p>{t("Рабочая папка и права выбираются в обычной форме запуска. Требуемые папки:")}{' '}{[...new Set(picked.map(comment => comment.root_id))].map(id => project.roots.find(item => item.id === id)?.name || t("Папка удалена")).join(', ') || root.name}.</p><button type="button" className="button secondary" onClick={launchNew}>{t("Выбрать агента и запустить сессию")}</button></div>}
      {target && !compatible && <div className="notice">{t("У сессии нет доступа ко всем выбранным папкам либо она завершена. Выберите другую сессию или настройте новую.")}</div>}
      <label>{t("Задача или сообщение")}{' '}<span className="muted">{t("необязательно")}</span><textarea rows={3} value={message} disabled={!!busy} onChange={event => setMessage(event.target.value)} placeholder={t("Например, Исправь выбранные замечания. Остальные изменения не трогай.")} /></label>
      <div className="comments-selection-tools"><button type="button" aria-pressed={!showAll} onClick={() => { setShowAll(false); setSelected(previous => new Set([...previous].filter(id => { const comment = candidates.find(item => item.id === id); return comment && (!targetId || commentDelivery(comment, targetId, []) !== 'handed_off') }))) }}>{t("Не отправлены этому агенту")}</button><button type="button" aria-pressed={showAll} onClick={() => setShowAll(true)}>{t("Все")}</button><button type="button" onClick={() => setSelected(new Set(shown.map(comment => comment.id)))}>{t("Выбрать все")}</button><button type="button" onClick={() => setSelected(new Set())}>{t("Снять выбор")}</button></div>
      <div className="batch-comment-list">{shown.map(comment => <label key={comment.id}><input type="checkbox" checked={selected.has(comment.id)} disabled={!!busy} onChange={event => setSelected(previous => { const next = new Set(previous); event.target.checked ? next.add(comment.id) : next.delete(comment.id); return next })} /><span><strong>{comment.path} · {commentRangeLabel(comment)}</strong><small>{comment.text}</small>{comment.stale && <small className="comment-stale">{t("Код изменился · обновите привязку перед отправкой")}</small>}</span></label>)}</div>
      {!candidates.length && <p className="muted small">{t("Можно подготовить задачу без комментариев.")}</p>}
      {picked.length > 32 && <div className="inline-error">{t("В одном пакете может быть не больше 32 комментариев.")}</div>}
      {utf8Length(message.trim()) > 4000 && <div className="inline-error">{t("Сообщение должно занимать не больше 4000 байт UTF-8.")}</div>}
      <div className="modal-actions"><button type="button" className="button secondary" disabled={!!busy} onClick={onClose}>{t("Отмена")}</button><button type="button" className="button primary" disabled={!!busy || !valid} onClick={() => void run('preview')}>{busy === 'preview' ? t("Собираем…") : t("Предпросмотр · {p0}", { p0: picked.length })}</button></div>
    </> : <>
      <div className="batch-context"><strong>{target?.title || t("Сессия")}</strong><span>{batch.comments.length}{' '}{t("комментариев · неизменяемый снимок пакета")}</span></div>
      <pre className="review-batch-preview">{batch.text}</pre>{batch.attachment_path && <p className="comments-dialog-help">{t("Файл пакета:")}{' '}<code>{batch.attachment_path}</code></p>}
      {status === 'draft' && <p className="comments-dialog-help">{t("Проверьте текст. Полный текст будет сохранён во вложении Markdown; в терминал вставится путь к пакету без автоматического Enter.")}</p>}
      {status === 'prepared' && <div className="success-notice">{t("Путь к файлу пакета вставлен в CLI. Проверьте его в терминале и нажмите Enter, когда агент готов принять задачу.")}</div>}
      {(status === 'uncertain' || status === 'preparing') && <div className="notice">{t("Приложение не может подтвердить результат вставки. Проверьте терминал; повторная автоматическая вставка отключена.")}</div>}
      {status === 'handed_off' && <div className="success-notice">{t("Вы подтвердили передачу пакета в CLI. Это не означает, что агент прочитал замечания или исправил их.")}</div>}
      {['prepared', 'uncertain'].includes(status || '') && <label className="batch-confirm-label"><input type="checkbox" checked={confirmed} disabled={!!busy} onChange={event => setConfirmed(event.target.checked)} /><span>{t("Я проверил терминал и отправил этот пакет в CLI")}</span></label>}
      <div className="modal-actions">{status === 'draft' && <button type="button" className="button secondary" disabled={!!busy} onClick={() => { setBatch(null); setConfirmed(false); setError('') }}>{t("Изменить состав")}</button>}<button type="button" className="button secondary" disabled={!!busy} onClick={onClose}>{t("Закрыть")}</button>{target && onOpenSession && <button type="button" className="button secondary" disabled={!!busy} onClick={() => { onClose(); onOpenSession(target) }}>{t("Открыть терминал")}</button>}{status === 'draft' && <button type="button" className="button primary" disabled={!!busy || !target || !isRunning(target)} onClick={() => void run('insert')}>{busy === 'insert' ? t("Вставляем…") : t("Вставить пакет в CLI")}</button>}{['prepared', 'uncertain'].includes(status || '') && <button type="button" className="button primary" disabled={!!busy || !confirmed} onClick={() => void run('confirm')}>{busy === 'confirm' ? t("Сохраняем…") : t("Подтвердить передачу в CLI")}</button>}</div>
    </>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
  </Modal>
}
