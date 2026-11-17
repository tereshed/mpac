import { useState } from 'react'
import type { Agent, Project, Session } from './api'
import { age, isRunning, statusLabel } from './api'
import { activitySummary, backgroundInventoryLabel, toolCallLabel, unreadResult } from './activity'
import { desktopSessionGroups, desktopSessionStatus } from './desktop-session-status'
import { orderedSessions } from './session-management'
import type { SessionView } from './session-management'
import ProjectRow from './ProjectRow'
import RailActionsMenu from './RailActionsMenu'
import { t } from './i18n.ts'
import './desktop-session-rail.css'

interface Props {
  projects: Project[]; sessions: Session[]; agents: Agent[]; selectedProject?: Project; selectedSession?: Session;
  expandedProjects: Record<string, boolean>; view: SessionView; queue: boolean;
  canManageProjects: boolean; canManageSessions: boolean; canRenameSessions: boolean; canSetLaunchDefaults: boolean;
  onToggleProject: (id: string, expanded: boolean) => void; onSelectProject: (id: string) => void;
  onSelectSession: (session: Session) => void; onLaunch: (project: Project) => void;
  onEditProject: (project: Project) => void; onDeleteProject: (project: Project) => void; onLaunchDefaults: (project: Project) => void;
  onProjectTools?: (project: Project) => void;
  onRenameSession: (session: Session) => void; onDeleteSession: (session: Session) => void;
}

export default function DesktopSessionRail({ projects, sessions, agents, selectedProject, selectedSession, expandedProjects, view, queue,
  canManageProjects, canManageSessions, canRenameSessions, canSetLaunchDefaults,
  onToggleProject, onSelectProject, onSelectSession, onLaunch, onEditProject, onDeleteProject, onLaunchDefaults, onProjectTools, onRenameSession, onDeleteSession,
}: Props) {
  const [otherOpen, setOtherOpen] = useState(false)
  const projectsById = new Map(projects.map(project => [project.id, project]))
  const ordered = orderedSessions(sessions)
  const sessionsByProject = new Map<string, Session[]>()
  for (const session of ordered) {
    const existing = sessionsByProject.get(session.project_id)
    if (existing) existing.push(session)
    else sessionsByProject.set(session.project_id, [session])
  }
  const agentName = (session: Session) => t(agents.find(agent => agent.id === session.agent)?.label || session.agent)
  const sessionRow = (session: Session, dotNumber?: number) => {
    const status = desktopSessionStatus(session)
    const glyph = desktopSessionGroups.find(group => group.id === status.id)?.glyph
    const projectName = projectsById.get(session.project_id)?.name
    const summary = activitySummary(session)
    const details = [session.title, projectName, agentName(session), statusLabel(session.status), summary.label, summary.detail,
      toolCallLabel(session), backgroundInventoryLabel(session), unreadResult(session) && summary.tone !== 'result' ? t('Есть непросмотренный результат') : ''].filter(Boolean).join(' · ')
    const selected = selectedSession?.id === session.id
    return <button key={session.id} type="button" className={dotNumber === undefined ? 'compact-session-row' : `session-orb compact-session-orb ${selected ? 'selected' : ''}`} title={details} aria-label={dotNumber === undefined ? undefined : details} aria-pressed={selected} onClick={() => onSelectSession(session)}>
      {dotNumber === undefined ? <>
        <span className={`compact-session-glyph compact-status-${status.id}`} aria-hidden="true">{glyph}</span>
        <span className="compact-session-name">{session.title}</span>
        {queue && <span className="compact-session-project" title={projectName}>{projectName}</span>}
        {(!queue || ['attention', 'error', 'background'].includes(status.id)) && <span className={`compact-session-status compact-status-${status.id}`}>{status.label}</span>}
      </> : <><span>{dotNumber}</span><span className={`activity-orb-badge compact-status-${status.id}`} aria-hidden="true">{glyph}</span></>}
    </button>
  }
  const projectRow = (project: Project, index: number) => {
    const projectSessions = sessionsByProject.get(project.id) || []
    const expanded = !!projectSessions.length && (expandedProjects[project.id] ?? true)
    return <div className="project-group" key={project.id}>
      <ProjectRow project={project} index={index} selected={project.id === selectedProject?.id} runningCount={projectSessions.filter(isRunning).length}
        expanded={expanded} onToggle={projectSessions.length ? () => onToggleProject(project.id, !expanded) : undefined}
        canManage={canManageProjects} onSelect={() => onSelectProject(project.id)} onEdit={() => onEditProject(project)} onDelete={() => onDeleteProject(project)}
        onLaunchDefaults={canSetLaunchDefaults ? () => onLaunchDefaults(project) : undefined} onProjectTools={onProjectTools && (() => onProjectTools(project))} onLaunch={() => onLaunch(project)} />
      {!!projectSessions.length && <div id={`sessions-${project.id}`} className={`compact-project-sessions ${view === 'dots' ? 'compact-session-dots' : ''}`} role="group" aria-label={t('Сессии проекта {p0}', { p0: project.name })} hidden={!expanded}>
        {expanded && projectSessions.map((session, i) => sessionRow(session, view === 'dots' ? i + 1 : undefined))}
      </div>}
    </div>
  }
  const withSessions = projects.filter(project => sessionsByProject.has(project.id))
  const other = projects.filter(project => !sessionsByProject.has(project.id))
  return <>
    <nav className={`project-list compact-project-list ${queue ? 'compact-status-queue' : ''}`} aria-label={t(queue ? 'Сессии по статусам' : 'Проекты и сессии')}>
      {queue ? <>
        {desktopSessionGroups.map(group => {
          const entries = ordered.filter(session => desktopSessionStatus(session).id === group.id)
          return !!entries.length && <section className="compact-queue-group" key={group.id} aria-label={t(group.label)}>
            <h3 className={`compact-status-${group.id}`}>{t(group.label)} <span>{entries.length}</span></h3>
            {entries.map(session => sessionRow(session))}
          </section>
        })}
        {!ordered.length && <p className="compact-queue-empty">{t('Сессий пока нет')}</p>}
      </> : <>
        {withSessions.map(projectRow)}
        {!!other.length && <>
          <button type="button" className="compact-other-projects" aria-expanded={otherOpen} aria-controls="compact-other-project-list" onClick={() => setOtherOpen(!otherOpen)}>
            <span aria-hidden="true">{otherOpen ? '⌄' : '›'}</span>{t('Другие проекты')}<span className="compact-other-count">{other.length}</span>
          </button>
          <div id="compact-other-project-list" className="compact-other-project-list" hidden={!otherOpen}>{otherOpen && other.map(projectRow)}</div>
        </>}
      </>}
    </nav>
    {selectedSession && <div className="compact-session-selection" role="group" aria-label={t('Выбранная сессия')}>
      <div className="compact-session-selection-copy">
        <strong title={selectedSession.title}>{selectedSession.title}</strong>
        <small title={t('Последняя активность')}>{agentName(selectedSession)} · {age(selectedSession.last_activity || selectedSession.started_at)}</small>
      </div>
      {(canRenameSessions || canManageSessions) && <RailActionsMenu key={selectedSession.id} id="compact-selected-session-actions" label={selectedSession.title}
        title={t('Действия с сессией')} triggerLabel={t('Действия с сессией {p0}', { p0: selectedSession.title })} actions={[
          ...(canRenameSessions ? [{ label: t('Переименовать сессию'), run: () => onRenameSession(selectedSession) }] : []),
          ...(canManageSessions ? [{ label: t('Удалить сессию из списка'), run: () => onDeleteSession(selectedSession), destructive: true }] : []),
        ]} />}
    </div>}
  </>
}
