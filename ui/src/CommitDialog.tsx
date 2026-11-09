import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage, query } from './api'
import type { Project, Review, Root } from './api'
import type { CommitPreview, CommitResult } from './commit-types'
import Modal from './Modal'
import { utf8Length } from './review-selection'
import './repository-toolbar.css'

interface Props { token: string; project: Project; root: Root; onClose: () => void; onBeforeMutation: () => void; onAfterMutation: (changed: boolean) => void }
export default function CommitDialog({ token, project, root, onClose, onBeforeMutation, onAfterMutation }: Props) {
  const [files, setFiles] = useState<Review['files']>([])
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [preview, setPreview] = useState<CommitPreview | null>(null)
  const [result, setResult] = useState<CommitResult | null>(null)
  const [message, setMessage] = useState('')
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<'preview' | 'commit' | null>(null)
  const [error, setError] = useState('')
  const [attempted, setAttempted] = useState(false)
  const [refresh, setRefresh] = useState(0)
  const mounted = useRef(true)
  const pending = useRef(false)
  const request = useRef<AbortController | null>(null)
  const requestId = useRef('')
  const context = token + '\0' + project.id + '\0' + root.id
  const latest = useRef(context); latest.current = context
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; request.current?.abort() } }, [])
  useEffect(() => {
    const abort = new AbortController(); setLoading(true); setError('')
    api<Review>(token, `/projects/${project.id}/review?${query({ root_id: root.id })}`, undefined, abort.signal)
      .then(review => { if (!abort.signal.aborted && latest.current === context) { setFiles(review.files); setSelected(previous => new Set([...previous].filter(path => review.files.some(file => file.path === path)))) } })
      .catch(failure => { if (!abort.signal.aborted && latest.current === context) setError(errorMessage(failure)) })
      .finally(() => { if (!abort.signal.aborted && latest.current === context) setLoading(false) })
    return () => abort.abort()
  }, [token, project.id, root.id, refresh])
  const getPreview = async () => {
    if (pending.current || !selected.size) return
    const abort = new AbortController(); request.current = abort; pending.current = true; setBusy('preview'); setError('')
    try {
      const value = await api<CommitPreview>(token, `/projects/${project.id}/commit-preview`, { root_id: root.id, paths: [...selected] }, abort.signal)
      if (mounted.current && !abort.signal.aborted && latest.current === context) { setPreview(value); requestId.current = crypto.randomUUID(); setAttempted(false) }
    } catch (failure) { if (mounted.current && !abort.signal.aborted && latest.current === context) setError(errorMessage(failure)) }
    finally { pending.current = false; if (request.current === abort) request.current = null; if (mounted.current) setBusy(null) }
  }
  const commit = async () => {
    if (!preview || preview.blocked_reason || !message.trim() || utf8Length(message.trim()) > 4000 || pending.current) return
    const abort = new AbortController(); request.current = abort; pending.current = true; setBusy('commit'); setError(''); setAttempted(true)
    onBeforeMutation()
    let changed = false
    try {
      const value = await api<CommitResult>(token, `/projects/${project.id}/commit`, { root_id: root.id, paths: preview.paths, expected_snapshot: preview.snapshot, message: message.trim(), request_id: requestId.current }, abort.signal)
      if (mounted.current && !abort.signal.aborted && latest.current === context) { changed = value.committed; setResult(value) }
    } catch (failure) { if (mounted.current && !abort.signal.aborted && latest.current === context) setError(errorMessage(failure)) }
    finally { pending.current = false; if (request.current === abort) request.current = null; if (mounted.current && latest.current === context) { setBusy(null); onAfterMutation(changed) } }
  }
  return <Modal title={result ? result.committed ? t("Коммит создан") : t("Результат коммита не определён") : t("Создать коммит")} subtitle={project.name + ' · ' + root.name + t(" · выбранные целые файлы")} busy={!!busy} onClose={() => { if (!pending.current) onClose() }}>
    {result ? <><div className={result.committed ? "commit-success" : "notice"}><strong>{result.head}</strong><p>{result.committed ? t("В коммит вошло файлов: {p0}. Остальные изменения сохраняются в рабочей папке и индексе.", { p0: result.paths.length }) : t("Git изменился во время выполнения. Приложение не подтверждает создание ожидаемого коммита. Проверьте состояние репозитория и предупреждение ниже перед новым действием.")}</p></div>{result.warning && <div className="notice">{t(result.warning)}</div>}{result.recovery_index && <div className="notice">{t("Сохранена резервная копия индекса:")}{' '}<code>{result.recovery_index}</code></div>}<div className="modal-actions"><button type="button" className="button primary" onClick={onClose}>{t("Готово")}</button></div></> : !preview ? <>
      <div className="commit-list-actions"><button type="button" disabled={loading || !!busy} onClick={() => setSelected(new Set(files.map(file => file.path)))}>{t("Выбрать все")}</button><button type="button" disabled={!!busy} onClick={() => setSelected(new Set())}>{t("Снять выбор")}</button><button type="button" disabled={loading || !!busy} onClick={() => setRefresh(value => value + 1)}>{t("Обновить")}</button></div>
      {loading ? <p className="comments-dialog-help">{t("Читаем состояние репозитория…")}</p> : <div className="commit-file-list">{files.map(file => <label key={file.path}><input type="checkbox" checked={selected.has(file.path)} disabled={!!busy} onChange={event => setSelected(previous => { const next = new Set(previous); event.target.checked ? next.add(file.path) : next.delete(file.path); return next })} /><span><strong>{file.path}</strong><small>{file.status === '??' ? t("Новый файл") : file.status}</small></span></label>)}</div>}
      {!loading && !files.length && <div className="notice">{t("Нет изменений для коммита.")}</div>}
      <p className="comments-dialog-help">{t("Сначала выберите состав коммита. Включается целый файл; частично подготовленные файлы требуют отдельной проверки.")}</p>
      <div className="modal-actions"><button type="button" className="button secondary" disabled={!!busy} onClick={onClose}>{t("Отмена")}</button><button type="button" className="button primary" disabled={loading || !!busy || !selected.size} onClick={() => void getPreview()}>{busy === 'preview' ? t("Собираем…") : t("Посмотреть выбранное · {p0}", { p0: selected.size })}</button></div>
    </> : <>
      <div className="commit-preview-context"><strong>{preview.branch || t("Без имени ветки")}</strong><span>{preview.paths.length}{' '}{t("файлов ·")}{' '}{preview.head === 'unborn' ? t("первый коммит") : preview.head.slice(0, 10)}</span></div>
      <div className="commit-preview-paths">{preview.paths.map(path => <code key={path}>{path}</code>)}</div>
      <pre className="commit-diff-preview">{preview.diff || t("Изменение без текстового diff")}</pre>
      {preview.blocked_reason && <div className="notice">{t(preview.blocked_reason)}</div>}
      <label>{t("Сообщение коммита")}<input value={message} onChange={event => setMessage(event.target.value)} disabled={!!busy || attempted} placeholder={t("Например, fix(studio): preserve cover title size")} /></label>
      {utf8Length(message.trim()) > 4000 && <div className="inline-error">{t("Сообщение должно занимать не больше 4000 байт UTF-8.")}</div>}
      <p className="comments-dialog-help">{t("Фиксируется просмотренный состав. Перед созданием Git повторно проверит состояние выбранных файлов и индекса.")}</p>
      {attempted && error && <div className="notice">{t("Результат запроса мог не дойти до интерфейса. Повторная проверка использует тот же идентификатор операции и не создаёт второй коммит.")}</div>}
      <div className="modal-actions"><button type="button" className="button secondary" disabled={!!busy} onClick={() => { setPreview(null); setError(''); setAttempted(false); setRefresh(value => value + 1) }}>{t("Обновить состав")}</button><button type="button" className="button secondary" disabled={!!busy} onClick={onClose}>{t("Закрыть")}</button><button type="button" className="button primary" disabled={!!busy || !!preview.blocked_reason || !message.trim() || utf8Length(message.trim()) > 4000} onClick={() => void commit()}>{busy === 'commit' ? t("Создаём…") : attempted ? t("Проверить результат операции") : t("Создать коммит")}</button></div>
    </>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
  </Modal>
}
