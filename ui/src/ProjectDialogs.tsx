import { t } from './i18n.ts'
import { folderLabel } from './project-management.ts'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from './api'
import type { Project, Session } from './api'
import Modal from './Modal'
import { activeProjectSessions } from './project-management'

export function DeleteProjectDialog({ token, project, sessions, enabled, onClose, onDeleted }: {
  token: string; project: Project; sessions: Session[]; enabled: boolean;
  onClose: () => void; onDeleted: (project: Project) => boolean;
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const submitting = useRef(false)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  const active = activeProjectSessions(project.id, sessions)
  const remove = async () => {
    if (submitting.current || active.length || !enabled) return
    submitting.current = true; setBusy(true); setError('')
    try {
      await api(token, `/projects/${project.id}/delete`, { expected: project })
      if (!onDeleted(project) && mounted.current) setError(t("Проект удалён из приложения, но подключение изменилось. Закройте окно и обновите список проектов."))
    } catch (failure) { if (mounted.current) setError(errorMessage(failure)) }
    finally { submitting.current = false; if (mounted.current) setBusy(false) }
  }
  return <Modal title={t("Удалить проект из приложения?")} subtitle={t("«{p0}» будет перемещён в удалённые проекты.", { p0: project.name })} onClose={onClose} busy={busy}>
    <div className="project-delete-summary"><strong>{t("Файлы на диске и история сессий сохранятся.")}</strong><p>{t("Проект можно будет восстановить из раздела «Удалённые проекты» даже после перезапуска приложения.")}</p><ul>{project.roots.map(root => <li key={root.id}><span>▱ {root.name}</span><code>{root.path}</code></li>)}</ul></div>
    {active.length > 0 && <div className="notice" role="status">{t("У проекта есть работающие сессии ({p0}). Удаление станет доступно после их завершения. Приложение не останавливает сессии при удалении проекта.", { p0: active.length })}<ul className="active-session-names">{active.slice(0, 4).map(session => <li key={session.id}>{session.title}</li>)}</ul></div>}
    {!enabled && <div className="notice">{t("Управление проектами пока недоступно в запущенном daemon.")}</div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={busy}>{t("Отмена")}</button><button type="button" className="button danger" onClick={remove} disabled={busy || active.length > 0 || !enabled}>{busy ? t("Удаляем…") : t("Удалить из приложения")}</button></div>
  </Modal>
}

export function ArchivedProjectsDialog({ projects, enabled, restoringId, onClose, onRestore }: {
  projects: Project[]; enabled: boolean; restoringId: string | null;
  onClose: () => void; onRestore: (project: Project) => Promise<Project>;
}) {
  const [error, setError] = useState('')
  const [restored, setRestored] = useState('')
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  const restore = async (project: Project) => {
    if (restoringId || !enabled) return
    setError(''); setRestored('')
    try { const saved = await onRestore(project); if (mounted.current) setRestored(saved.name) }
    catch (failure) { if (mounted.current) setError(errorMessage(failure)) }
  }
  return <Modal title={t("Удалённые проекты")} subtitle={t("Здесь можно вернуть проект в рабочее пространство. Файлы на диске и история сессий сохранены.")} onClose={onClose} busy={!!restoringId}>
    {restored && <div className="success-notice" role="status">{t("Проект «{p0}» восстановлен.", { p0: restored })}</div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    {!enabled && <div className="notice">{t("Восстановление пока недоступно в запущенном daemon.")}</div>}
    <div className="archived-projects-list">{projects.length ? projects.map(project => <div className="archived-project-row" key={project.id}><span className="project-avatar">{project.name.slice(0, 1).toUpperCase()}</span><div><strong>{project.name}</strong><span>{project.roots.length} {folderLabel(project.roots.length)}</span></div><button type="button" className="button secondary compact" disabled={!!restoringId || !enabled} onClick={() => void restore(project)}>{restoringId === project.id ? t("Восстанавливаем…") : t("Восстановить")}</button></div>) : <div className="archived-projects-empty"><span>▱</span><p>{t("Удалённых проектов пока нет.")}</p></div>}</div>
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={!!restoringId}>{t("Закрыть")}</button></div>
  </Modal>
}
