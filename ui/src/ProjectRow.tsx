import { t } from './i18n.ts'
import { folderLabel } from './project-management.ts'
import { useLayoutEffect, useRef } from 'react'
import RailActionsMenu from './RailActionsMenu'
import type { Project } from './api'

export default function ProjectRow({ project, index, selected, runningCount, canManage, onSelect, onEdit, onDelete, onLaunchDefaults, onProjectTools, expanded, onToggle, sessionCount = 0, onLaunch }: {
  project: Project; index: number; selected: boolean; runningCount: number; canManage: boolean;
  onSelect: () => void; onEdit: () => void; onDelete: () => void;
  expanded?: boolean; onToggle?: () => void; sessionCount?: number;
  onLaunchDefaults?: () => void; onProjectTools?: () => void; onLaunch?: () => void;
}) {
  const container = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    if (!selected) return
    const group = container.current?.parentElement
    const list = group?.closest('.project-list')
    if (!group || !list) return
    const itemBounds = group.getBoundingClientRect(), listBounds = list.getBoundingClientRect()
    if (itemBounds.top < listBounds.top) list.scrollTop -= listBounds.top - itemBounds.top
    else if (itemBounds.bottom > listBounds.bottom) list.scrollTop += Math.min(itemBounds.bottom - listBounds.bottom, itemBounds.top - listBounds.top)
  }, [selected, expanded])
  const compact = !!onLaunch
  return <div className={`project-row ${selected ? 'selected' : ''}`} ref={container}>
    {onToggle && <button type="button" className="project-expand" onClick={onToggle} aria-label={t("{p0} сессии проекта {p1}", { p0: expanded ? t("Свернуть") : t("Раскрыть"), p1: project.name })} aria-expanded={expanded} aria-controls={`sessions-${project.id}`}>{expanded ? '⌄' : '›'}</button>}
    <button className={`project-item ${selected ? 'active' : ''}`} onClick={onSelect} title={compact ? project.name : undefined} aria-current={selected ? 'page' : undefined}>
      {!compact && <span className={`project-avatar hue-${index % 4}`}>{project.name.slice(0, 1).toUpperCase()}</span>}
      <span className="project-item-name">{project.name}{!compact && <small>{project.roots.length} {folderLabel(project.roots.length)} · {sessionCount}{' '}{t("сесс.")}</small>}</span>
      <span className="project-session-count" title={t("Работающие сессии")}>{runningCount || ''}</span>
    </button>
    {onLaunch && <button type="button" className="compact-project-launch" onClick={onLaunch} aria-label={t("Новая сессия в проекте {p0}", { p0: project.name })} title={t("Новая сессия")}>+</button>}
    {canManage && <RailActionsMenu id={`project-actions-${project.id}`} label={t("Проект {p0}", { p0: project.name })} title={t("Действия с проектом")} triggerLabel={t("Действия с проектом {p0}", { p0: project.name })} actions={[
      { label: t("Изменить проект"), run: onEdit },
      ...(onLaunchDefaults ? [{ label: t("Настройки запуска агентов"), run: onLaunchDefaults }] : []),
      ...(onProjectTools ? [{ label: t("MCP и навыки"), run: onProjectTools }] : []),
      { label: t("Удалить из приложения…"), run: onDelete, destructive: true },
    ]} />}
  </div>
}
