import { t } from './i18n.ts'
import { age, statusLabel } from './api'
import type { Agent, Project, Session } from './api'
import { orderedSessions } from './session-management'
import type { SessionView } from './session-management'
import { activitySummary, backgroundInventoryLabel, toolCallLabel, unreadResult } from './activity'
import './activity.css'

const activityGlyph = { attention: '?', result: '●', working: '↻', idle: '·', unknown: '?', stopped: '■' }

export default function ProjectSessions({ project, sessions, agents, selectedId, view, attentionOnly, canManage, onSelect, onDelete, onLaunch }: {
  project: Project; sessions: Session[]; agents: Agent[]; selectedId: string; view: SessionView; attentionOnly: boolean; canManage: boolean;
  onSelect: (session: Session) => void; onDelete: (session: Session) => void; onLaunch: () => void;
}) {
  const listed = orderedSessions(sessions, attentionOnly)
  const summary = (session: Session) => `${session.title} · ${t(agents.find(a => a.id === session.agent)?.label || session.agent)} · CLI: ${statusLabel(session.status)} · ${activitySummary(session).label}${toolCallLabel(session) ? ` · ${toolCallLabel(session)}` : ''}${backgroundInventoryLabel(session) ? ` · ${backgroundInventoryLabel(session)}` : ''}${unreadResult(session) ? t(" · Есть непросмотренный результат") : ''} · ${project.roots.find(r => r.id === session.root_id)?.name || t("Папка больше не в проекте")}`
  return <div className="project-sessions" id={`sessions-${project.id}`} role="group" aria-label={t("Сессии проекта {p0}", { p0: project.name })}>
    <div className="project-sessions-body"><div className={`nested-session-list ${view === 'dots' ? 'session-dot-grid' : ''}`}>
      {listed.map((session, index) => view === 'dots'
        ? <div key={session.id} className="session-orb-wrapper"><button type="button" className={`session-orb status-orb-${session.status} activity-orb-${activitySummary(session).tone} ${selectedId === session.id ? 'selected' : ''}`} title={summary(session)} aria-label={summary(session)} aria-pressed={selectedId === session.id} onClick={() => onSelect(session)}><span>{index + 1}</span><span className={`activity-orb-badge activity-${activitySummary(session).tone}`}>{activityGlyph[activitySummary(session).tone]}</span></button>{canManage && <button type="button" className="session-orb-delete" title={t("Удалить сессию {p0}: {p1}", { p0: index + 1, p1: session.title })} aria-label={t("Удалить сессию {p0}: {p1}", { p0: index + 1, p1: session.title })} onClick={() => onDelete(session)}>×</button>}</div>
        : <div key={session.id} className={`nested-session-row ${selectedId === session.id ? 'selected' : ''}`}>
          <button className="nested-session-select" onClick={() => onSelect(session)} title={summary(session)} aria-pressed={selectedId === session.id}>
            <span className={`status-dot status-${session.status}`} />
            <span className="nested-session-copy"><strong>{session.title}</strong><small>{t(agents.find(a => a.id === session.agent)?.label || session.agent)} · CLI: {statusLabel(session.status)}</small><span className={`activity-state activity-${activitySummary(session).tone}`} title={activitySummary(session).detail}><i className="activity-state-dot" />{activitySummary(session).label}</span>{toolCallLabel(session) && <small className="session-tool-detail">{toolCallLabel(session)}</small>}{backgroundInventoryLabel(session) && <small className="session-background-detail">{backgroundInventoryLabel(session)}</small>}</span>
            <span className="nested-session-age">{age(session.started_at)}</span>
          </button>
          {canManage && <button className="session-delete-trigger" title={t("Удалить сессию из списка")} aria-label={t("Удалить сессию {p0}", { p0: session.title })} onClick={() => onDelete(session)}>×</button>}
        </div>)}
    </div><button type="button" className="nested-session-new" onClick={onLaunch} aria-label={t("Новая сессия в проекте {p0}", { p0: project.name })} title={t("Новая сессия")}>+</button></div>
    {!listed.length && <p className="nested-session-empty">{attentionOnly ? t("Нет запросов внимания") : t("Сессий пока нет")}</p>}
    {view === 'dots' && !!listed.length && <p className="session-dots-hint">{t("Выберите кружок, чтобы открыть сессию")}</p>}
  </div>
}
