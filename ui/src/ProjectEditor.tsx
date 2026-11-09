import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { api, errorMessage } from './api'
import type { Project, Session } from './api'
import DirectoryPicker from './DirectoryPicker'
import Modal from './Modal'
import { missingProtectedRoots, projectPaths, protectedProjectRoots } from './project-management'

export default function ProjectEditor({ token, expected, sessions, managementAvailable, onClose, onSaved }: {
  token: string; expected?: Project; sessions: Session[]; managementAvailable: boolean;
  onClose: () => void; onSaved: (project: Project) => boolean;
}) {
  const [name, setName] = useState(expected?.name || '')
  const [selectedPaths, setSelectedPaths] = useState<string[]>(expected?.roots.map(root => root.path) || [])
  const [manualPaths, setManualPaths] = useState('')
  const [picking, setPicking] = useState(false)
  const [browseStart, setBrowseStart] = useState<string>(expected?.roots.at(-1)?.path || '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const submitting = useRef(false)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  const paths = projectPaths(selectedPaths, manualPaths)
  const locked = expected ? protectedProjectRoots(expected, sessions) : []
  const missing = expected ? missingProtectedRoots(expected, paths, sessions) : []
  const canEdit = !expected || managementAvailable
  const valid = !!name.trim() && paths.length > 0 && paths.length <= 12 && !missing.length && canEdit
  const remove = (path: string) => {
    if (busy || locked.some(root => root.path === path)) return
    setSelectedPaths(previous => previous.filter(value => value !== path))
    setManualPaths(previous => previous.split('\n').filter(value => value.trim() !== path).join('\n'))
    setError('')
  }
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (submitting.current || !valid) return
    submitting.current = true; setBusy(true); setError('')
    try {
      const request = { name: name.trim(), paths, ...(expected ? { expected } : {}) }
      const saved = await api<Project>(token, expected ? `/projects/${expected.id}/update` : '/projects', request)
      if (!onSaved(saved) && mounted.current) setError(t("Проект сохранён, но подключение изменилось. Закройте окно и обновите список проектов."))
    } catch (failure) { if (mounted.current) setError(errorMessage(failure)) }
    finally { submitting.current = false; if (mounted.current) setBusy(false) }
  }
  const close = () => { if (busy) return; if (picking) setPicking(false); else onClose() }
  return <Modal title={picking ? t("Выбрать папку") : expected ? t("Изменить проект") : t("Добавить проект")} subtitle={picking ? t("Найдите папку проекта. Можно добавить несколько папок по очереди.") : expected ? t("Измените название и набор папок. Файлы на диске сохраняются.") : t("Объедините несколько локальных папок в одно рабочее пространство.")} onClose={close} busy={busy}>
    {picking ? <DirectoryPicker token={token} initialPath={browseStart} selectedPaths={paths} onCancel={() => setPicking(false)} onSelect={selection => {
      setSelectedPaths(previous => [...new Set([...previous, selection.path])]); setBrowseStart(selection.parent || selection.path); setPicking(false); setError('')
    }} /> : <form onSubmit={submit}>
      <label>{t("Название проекта")}<input autoFocus value={name} onChange={event => setName(event.target.value)} placeholder={t("Например, Platform")} maxLength={100} required disabled={busy} /></label>
      <div className="project-folders-heading"><label>{t("Папки проекта")}{' '}<span>{paths.length} / 12</span></label><button type="button" className="button secondary compact" onClick={() => setPicking(true)} disabled={busy || paths.length >= 12}>＋ {paths.length ? t("Добавить папку") : t("Выбрать папку")}</button></div>
      <div className="project-folder-list">{paths.length ? paths.map((path, index) => {
        const protectedRoot = locked.some(root => root.path === path)
        return <div className={`project-folder-item ${protectedRoot ? 'project-folder-protected' : ''}`} key={path}><span aria-hidden="true">▱</span><div><strong>{path.split(/[\\/]/).filter(Boolean).pop() || path}</strong><code title={path}>{path}</code>{protectedRoot && <small id={`protected-project-root-${index}`}>{t("Используется работающей сессией")}</small>}</div><button type="button" className="icon-button" disabled={busy || protectedRoot} onClick={() => remove(path)} aria-label={t("Удалить папку {p0}", { p0: path })} aria-describedby={protectedRoot ? `protected-project-root-${index}` : undefined} title={protectedRoot ? t("Папку нельзя убрать, пока сессия работает") : t("Удалить папку из проекта")}>×</button></div>
      }) : <button type="button" className="project-folder-empty" onClick={() => setPicking(true)} disabled={busy}><span aria-hidden="true">▱</span><span>{t("Выберите папку на компьютере")}</span><small>{t("Добавьте одну или несколько папок проекта")}</small></button>}</div>
      <details className="manual-paths"><summary>{t("Ввести пути вручную")}</summary><label htmlFor="manual-project-paths">{t("Абсолютные пути, по одному на строку")}<textarea id="manual-project-paths" value={manualPaths} onChange={event => { setManualPaths(event.target.value); setError('') }} placeholder={'/Users/me/Projects/backend\n/Users/me/Projects/frontend'} rows={3} disabled={busy} /></label></details>
      <p className="muted small">{t("CLI-сессия запускается в выбранной папке; файлы не перемещаются.")}{locked.length > 0 && t(" Название можно менять и добавлять папки, пока сессии работают.")}{expected && t(" Добавление папки в проект не расширяет права уже запущенных сессий.")}</p>
      {paths.length > 12 && <div className="inline-error" role="alert">{t("В проект можно добавить не более 12 папок. Удалите лишние пути.")}</div>}
      {!canEdit && <div className="notice">{t("Изменение проектов появится после обновления локального daemon. Работающие сессии можно продолжать.")}</div>}
      {missing.length > 0 && <div className="notice">{t("Папки стали использоваться работающей сессией, их нельзя убрать:")}{' '}{missing.map(root => root.name).join(', ')}.<button type="button" className="text-button" onClick={() => setSelectedPaths(previous => [...new Set([...previous, ...missing.map(root => root.path)])])}>{t("Вернуть используемые папки")}</button></div>}
      {error && <div className="inline-error" role="alert">{t(error)}</div>}
      <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose} disabled={busy}>{t("Отмена")}</button><button className="button primary" disabled={busy || !valid}>{busy ? t("Сохраняем…") : expected ? t("Сохранить изменения") : t("Добавить проект")}</button></div>
    </form>}
  </Modal>
}
