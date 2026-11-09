import { isRunning } from './api.ts'
import { localeTag, t } from './i18n.ts'
import type { Project, Root, Session } from './api'

/** The count is UI text; project names and filesystem identities stay literal. */
export function folderLabel(count: number): string {
  const category = new Intl.PluralRules(localeTag()).select(count)
  return t(category === 'one' ? 'папка' : category === 'many' ? 'папок' : 'папки')
}

export function projectPaths(selected: string[], manual: string): string[] {
  // Filesystem-selected paths are canonical identities, including spaces.
  return [...new Set([...selected, ...manual.split('\n').map(path => path.trim()).filter(Boolean)])]
}

export function activeProjectSessions(projectId: string, sessions: Session[]): Session[] {
  return sessions.filter(session => session.project_id === projectId && isRunning(session))
}

export function protectedProjectRoots(project: Project, sessions: Session[]): Root[] {
  const ids = new Set(activeProjectSessions(project.id, sessions).flatMap(session => [session.root_id, ...(session.additional_roots || []).map(root => root.id)]))
  return project.roots.filter(root => ids.has(root.id))
}

export function missingProtectedRoots(project: Project, nextPaths: string[], sessions: Session[]): Root[] {
  return protectedProjectRoots(project, sessions).filter(root => !nextPaths.includes(root.path))
}

export function reconcileProjectSelection(project: Project, rootId: string, sessionId: string, sessions: Session[]): { rootId: string; sessionId: string } {
  const root = project.roots.find(item => item.id === rootId) || project.roots[0]
  const selected = sessions.find(session => session.id === sessionId && session.project_id === project.id)
  // Preserve an active execution's selection. Removed historic folders stay
  // in history, but editing the project doesn't silently select that folder.
  const keepSession = selected && (project.roots.some(item => item.id === selected.root_id) || isRunning(selected))
  return { rootId: root?.id || '', sessionId: keepSession ? selected.id : '' }
}
