import { t } from './i18n.ts'
import { folderLabel } from './project-management.ts'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import type { Project } from './api'

export default function ProjectRow({ project, index, selected, runningCount, canManage, onSelect, onEdit, onDelete, onLaunchDefaults, expanded, onToggle, sessionCount = 0 }: {
  project: Project; index: number; selected: boolean; runningCount: number; canManage: boolean;
  onSelect: () => void; onEdit: () => void; onDelete: () => void;
  expanded?: boolean; onToggle?: () => void; sessionCount?: number;
  onLaunchDefaults?: () => void;
}) {
  const [open, setOpen] = useState(false)
  const container = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState({ left: 8, top: 8 })
  const startAtEnd = useRef(false)
  const menuId = `project-actions-${project.id}`
  useLayoutEffect(() => {
    if (!selected) return
    const group = container.current?.parentElement
    const list = group?.parentElement
    if (!group || !list) return
    const itemBounds = group.getBoundingClientRect(), listBounds = list.getBoundingClientRect()
    if (itemBounds.top < listBounds.top) list.scrollTop -= listBounds.top - itemBounds.top
    else if (itemBounds.bottom > listBounds.bottom) list.scrollTop += Math.min(itemBounds.bottom - listBounds.bottom, itemBounds.top - listBounds.top)
  }, [selected, expanded])
  const reveal = (atEnd = false) => {
    const rect = trigger.current?.getBoundingClientRect()
    if (!rect) return
    const menuHeight = onLaunchDefaults ? 140 : 100
    setPosition({ left: Math.max(8, Math.min(window.innerWidth - 218, rect.right - 210)), top: rect.bottom + menuHeight < window.innerHeight ? rect.bottom + 5 : Math.max(8, rect.top - menuHeight) })
    startAtEnd.current = atEnd; setOpen(true)
  }
  useEffect(() => {
    if (!canManage) { setOpen(false); return }
    if (!open) return
    const items = () => [...(menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') || [])]
    const initial = items()
    initial[startAtEnd.current ? initial.length - 1 : 0]?.focus()
    const contains = (target: Node | null) => container.current?.contains(target) || menu.current?.contains(target)
    const outside = (event: PointerEvent) => { if (!contains(event.target as Node)) setOpen(false) }
    const closeOnLayout = (event: Event) => { if (!menu.current?.contains(event.target as Node)) setOpen(false) }
    const keyboard = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); setOpen(false); trigger.current?.focus(); return }
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return
      if (!contains(document.activeElement)) return
      event.preventDefault()
      const list = items()
      const current = list.indexOf(document.activeElement as HTMLButtonElement)
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? list.length - 1 : (current + (event.key === 'ArrowDown' ? 1 : -1) + list.length) % list.length
      list[next]?.focus()
    }
    document.addEventListener('pointerdown', outside)
    document.addEventListener('keydown', keyboard)
    document.addEventListener('scroll', closeOnLayout, true)
    window.addEventListener('resize', closeOnLayout)
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', keyboard); document.removeEventListener('scroll', closeOnLayout, true); window.removeEventListener('resize', closeOnLayout) }
  }, [open, canManage])
  return <div className={`project-row ${selected ? 'selected' : ''}`} ref={container} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null) && !menu.current?.contains(event.relatedTarget as Node | null)) setOpen(false) }}>
    {onToggle && <button type="button" className="project-expand" onClick={onToggle} aria-label={t("{p0} сессии проекта {p1}", { p0: expanded ? t("Свернуть") : t("Раскрыть"), p1: project.name })} aria-expanded={expanded} aria-controls={`sessions-${project.id}`}>{expanded ? '⌄' : '›'}</button>}
    <button className={`project-item ${selected ? 'active' : ''}`} onClick={() => { setOpen(false); onSelect() }} aria-current={selected ? 'page' : undefined}><span className={`project-avatar hue-${index % 4}`}>{project.name.slice(0, 1).toUpperCase()}</span><span className="project-item-name">{project.name}<small>{project.roots.length} {folderLabel(project.roots.length)} · {sessionCount}{' '}{t("сесс.")}</small></span><span className="project-session-count" title={t("Работающие сессии")}>{runningCount || ''}</span></button>
    {canManage && <button type="button" className="project-actions-trigger" ref={trigger} aria-label={t("Действия с проектом {p0}", { p0: project.name })} title={t("Действия с проектом")} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined} onClick={() => { if (open) setOpen(false); else reveal() }} onKeyDown={event => { if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); reveal(event.key === 'ArrowUp') } }}>⋯</button>}
    {canManage && open && createPortal(<div ref={menu} className="project-actions-menu" style={position} role="menu" id={menuId} aria-label={t("Проект {p0}", { p0: project.name })}><button type="button" role="menuitem" tabIndex={-1} onClick={() => { setOpen(false); onEdit() }}>{t("Изменить проект")}</button>{onLaunchDefaults && <button type="button" role="menuitem" tabIndex={-1} onClick={() => { setOpen(false); onLaunchDefaults() }}>{t("Настройки запуска агентов")}</button>}<button type="button" role="menuitem" tabIndex={-1} className="project-delete-action" onClick={() => { setOpen(false); onDelete() }}>{t("Удалить из приложения…")}</button></div>, document.body)}
  </div>
}
