import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { api, errorMessage, isRunning, statusLabel } from './api'
import type { Project, Session } from './api'
import Modal from './Modal'
import { sessionTitle } from './session-management'

export function RenameSessionDialog({ token, session, sessions, enabled, onClose, onSaved }: {
  token: string; session: Session; sessions: Session[]; enabled: boolean;
  onClose: () => void; onSaved: (session: Session) => boolean;
}) {
  const [title, setTitle] = useState(session.title)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const submitting = useRef(false)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  const current = sessions.find(item => item.id === session.id)
  const candidate = current?.cli_title || ''
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (submitting.current || !enabled || !current) return
    let valid: string
    try { valid = sessionTitle(title) } catch (failure) { setError(errorMessage(failure)); return }
    submitting.current = true; setBusy(true); setError('')
    try {
      const saved = await api<Session>(token, `/sessions/${session.id}/rename`, { title: valid, expected_title: session.title })
      if (!onSaved(saved) && mounted.current) setError(t("Название сохранено, но подключение изменилось. Закройте окно и обновите список сессий."))
    } catch (failure) { if (mounted.current) setError(errorMessage(failure)) }
    finally { submitting.current = false; if (mounted.current) setBusy(false) }
  }
  return <Modal title={t("Название сессии")} subtitle={t("Изменение названия не прерывает работу CLI.")} onClose={onClose} busy={busy}><form onSubmit={submit}>
    <label>{t("Название")}<input autoFocus value={title} disabled={busy || !enabled || !current} onChange={event => { setTitle(event.target.value); setError('') }} autoComplete="off" /></label>
    <div className="cli-title-candidate"><span>{t("Заголовок CLI")}</span>{candidate ? <><p>{candidate}</p><button type="button" className="button secondary compact" disabled={busy || !enabled || !current} onClick={() => { setTitle(candidate); setError('') }}>{t("Использовать название из CLI")}</button></> : <p>{t("CLI пока не передал заголовок. Название можно задать вручную.")}</p>}<small>{t("Заголовок может включать статус работы и имя проекта. Проверьте его перед сохранением.")}</small></div>
    {!current && <div className="notice">{t("Сессия больше недоступна в текущем списке. Закройте окно и обновите данные.")}</div>}
    {!enabled && <div className="notice">{t("Переименование пока недоступно в запущенном daemon.")}</div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={busy}>{t("Отмена")}</button><button type="submit" className="button primary" disabled={busy || !enabled || !current || !title.trim()}>{busy ? t("Сохраняем…") : t("Сохранить название")}</button></div>
  </form></Modal>
}

export function DeleteSessionDialog({ token, session, sessions, enabled, onClose, onDeleted, onStopped }: {
  token: string; session: Session; sessions: Session[]; enabled: boolean;
  onClose: () => void; onDeleted: (session: Session) => boolean; onStopped: () => Promise<void>;
}) {
  const current = sessions.find(item => item.id === session.id) || session
  const active = isRunning(current)
  const phase = active ? 'stop' : 'delete'
  const [confirmedFor, setConfirmedFor] = useState<'stop' | 'delete' | null>(null)
  const confirmed = confirmedFor === phase
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const submitting = useRef(false)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  const act = async () => {
    if (submitting.current || !confirmed || !enabled) return
    submitting.current = true; setBusy(true); setError('')
    try {
      if (active) {
        await api(token, `/sessions/${session.id}/stop`, {})
        await onStopped()
        if (mounted.current) setConfirmedFor(null)
      } else {
        await api(token, `/sessions/${session.id}/delete`, {})
        if (!onDeleted(current) && mounted.current) setError(t("Сессия удалена, но подключение изменилось. Закройте окно и обновите список."))
      }
    } catch (failure) { if (mounted.current) setError(errorMessage(failure)) }
    finally { submitting.current = false; if (mounted.current) setBusy(false) }
  }
  return <Modal title={active ? t("Сначала остановить сессию?") : t("Удалить сессию из списка?")} subtitle={`«${session.title}» · ${statusLabel(current.status)}`} onClose={onClose} busy={busy}>
    <div className="session-delete-description">{active ? <p>{t("Сессия ещё работает. Остановка завершит CLI-процесс и текущую работу. После остановки потребуется отдельное подтверждение удаления.")}</p> : <p>{t("Сессия переместится в «Удалённые сессии». Её можно будет вернуть в список. Файлы проекта и изменения на диске сохранятся.")}</p>}</div>
    <label className="session-confirm"><input type="checkbox" checked={confirmed} disabled={busy || !enabled} onChange={event => setConfirmedFor(event.target.checked ? phase : null)} /><span>{active ? t("Подтверждаю остановку этой сессии") : t("Подтверждаю удаление этой сессии из списка")}</span></label>
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={busy}>{t("Отмена")}</button><button type="button" className="button danger" disabled={busy || !confirmed || !enabled} onClick={() => void act()}>{busy ? t("Выполняем…") : active ? t("Остановить сессию") : t("Удалить сессию")}</button></div>
  </Modal>
}

export function ArchivedSessionsDialog({ sessions, projects, restoringId, onClose, onRestore }: {
  sessions: Session[]; projects: Project[]; restoringId: string | null;
  onClose: () => void; onRestore: (session: Session) => Promise<void>;
}) {
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  const restore = async (session: Session) => {
    setError(''); setNotice('')
    try { await onRestore(session); if (mounted.current) setNotice(t("Сессия «{p0}» возвращена в список.", { p0: session.title })) }
    catch (failure) { if (mounted.current) setError(errorMessage(failure)) }
  }
  return <Modal title={t("Удалённые сессии")} subtitle={t("Восстановление возвращает запись в список. CLI-процесс автоматически не запускается.")} onClose={onClose} busy={!!restoringId}>
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    {notice && <div className="success-notice" role="status">{t(notice)}</div>}
    <div className="archived-projects-list">{sessions.length ? sessions.map(session => {
      const project = projects.find(item => item.id === session.project_id)
      return <div className="archived-project-row" key={session.id}><span className={`status-dot status-${session.status}`} /><div><strong>{session.title}</strong><span>{project?.name || t("Проект удалён")} · {statusLabel(session.status)}</span>{!project && <span>{t("Сначала восстановите проект")}</span>}</div><button type="button" className="button secondary compact" disabled={!!restoringId || !project} onClick={() => void restore(session)}>{restoringId === session.id ? t("Восстанавливаем…") : t("Восстановить")}</button></div>
    }) : <div className="archived-projects-empty">{t("Удалённых сессий пока нет.")}</div>}</div>
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={!!restoringId}>{t("Закрыть")}</button></div>
  </Modal>
}
