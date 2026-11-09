import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { CSSProperties } from 'react'
import type { Agent, Project, Session } from './api'
import { statusLabel } from './api'
import { activitySummary, activityTime, backgroundInventoryLabel, toolCallLabel, unreadResult } from './activity'
import { mobileAttentionEntries, mobileAttentionReasons, mobileSessions, mobileSessionMatches, mobileSessionTone, mobileCount } from './mobile-navigation'
import type { MobileSessionFilter, MobileSheet } from './mobile-navigation'
import './mobile-navigation.css'
import LanguageSwitcher from './LanguageSwitcher'

export type { MobileSheet } from './mobile-navigation'
interface Props {
  projects: Project[]; sessions: Session[]; agents: Agent[]; projectId: string; selectedSessionId: string; tab: 'terminal' | 'files' | 'review'
  sheet: MobileSheet; onSheetChange: (sheet: MobileSheet) => void; onSelectSession: (session: Session) => void; onSelectProject: (project: Project) => void
  onTerminal: () => void; onLaunch: (project: Project) => void; onAddProject: () => void; onManage: () => void
  disabled?: boolean; keyboardOpen?: boolean; onDockHeight?: (height: number) => void
}
const filters: { id: MobileSessionFilter; label: string }[] = [{ id: 'all', label: 'Все' }, { id: 'input', label: 'Ввод' }, { id: 'results', label: 'Результаты' }, { id: 'working', label: 'Работают' }, { id: 'background', label: 'Фон' }, { id: 'history', label: 'История' }]
const sheetTitles = { attention: 'Очередь действий', sessions: 'Сессии', projects: 'Проекты' }
const toneGlyph = { attention: '?', error: '!', result: '●', working: '↻', background: '◷', idle: '·', history: '■', unknown: '?' }
const focusSelector = 'button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],[tabindex]:not([tabindex="-1"])'

export default function MobileNavigation({ projects, sessions, agents, projectId, selectedSessionId, tab, sheet, onSheetChange, onSelectSession, onSelectProject, onTerminal, onLaunch, onAddProject, onManage, disabled = false, keyboardOpen = false, onDockHeight }: Props) {
  const shell = useRef<HTMLDivElement>(null), dock = useRef<HTMLDivElement>(null), panel = useRef<HTMLElement>(null)
  const controls = useRef({ sheet, disabled, onSheetChange }); controls.current = { sheet, disabled, onSheetChange }
  const heightCallback = useRef(onDockHeight); heightCallback.current = onDockHeight
  const [dockHeight, setDockHeight] = useState(0)
  const [scope, setScope] = useState<string | null>(null)
  const [filter, setFilter] = useState<MobileSessionFilter>('all')
  const scopeProject = projects.find(project => project.id === scope)
  const scopedId = scopeProject?.id || null
  const current = sessions.find(session => session.id === selectedSessionId)
  const currentProject = projects.find(project => project.id === (current?.project_id || projectId))
  const queue = mobileAttentionEntries(sessions)
  const listed = mobileSessions(sessions, scopedId, filter)
  const scopedSessions = sessions.filter(session => !scopedId || session.project_id === scopedId)
  const close = () => onSheetChange(null)
  const selectSession = (session: Session) => { close(); onSelectSession(session) }
  const launch = (project: Project) => { close(); onLaunch(project) }
  const focusTrigger = (button: HTMLButtonElement) => button.focus({ preventScroll: true })

  useEffect(() => {
    const element = dock.current
    if (!element) return
    let last = -1
    const measure = () => {
      const height = Math.ceil(element.getBoundingClientRect().height)
      if (height === last || height <= 0) return
      last = height; setDockHeight(height); heightCallback.current?.(height)
    }
    measure()
    const observer = new ResizeObserver(measure); observer.observe(element)
    return () => observer.disconnect()
  }, [])

  useEffect(() => {
    if (!sheet || disabled) return
    const previous = document.activeElement
    const focusable = () => [...(shell.current?.querySelectorAll<HTMLElement>(focusSelector) || [])].filter(element => element.getClientRects().length > 0 && !element.closest('[inert]'))
    const focusFirst = () => {
      if (controls.current.disabled || !controls.current.sheet) return
      ;(panel.current?.querySelector<HTMLElement>(focusSelector) || focusable()[0])?.focus({ preventScroll: true })
    }
    const frame = requestAnimationFrame(focusFirst)
    const keyboard = (event: KeyboardEvent) => {
      if (!controls.current.sheet || controls.current.disabled) return
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); controls.current.onSheetChange(null); return }
      if (event.key !== 'Tab') return
      const items = focusable(), first = items[0], last = items.at(-1)
      if (!first || !last) { event.preventDefault(); return }
      if (event.shiftKey && (document.activeElement === first || !shell.current?.contains(document.activeElement))) { event.preventDefault(); last.focus() }
      else if (!event.shiftKey && (document.activeElement === last || !shell.current?.contains(document.activeElement))) { event.preventDefault(); first.focus() }
    }
    const focus = (event: FocusEvent) => {
      if (!controls.current.sheet || controls.current.disabled || !(event.target instanceof Node) || shell.current?.contains(event.target)) return
      focusFirst()
    }
    document.addEventListener('keydown', keyboard)
    document.addEventListener('focusin', focus)
    return () => {
      cancelAnimationFrame(frame)
      document.removeEventListener('keydown', keyboard); document.removeEventListener('focusin', focus)
      // Never restore the CLI textarea: switching sheets must not summon IME.
      if (previous instanceof HTMLButtonElement && previous.isConnected && !previous.disabled && !previous.closest('[inert]') && (!document.activeElement || document.activeElement === document.body || shell.current?.contains(document.activeElement))) previous.focus({ preventScroll: true })
    }
  }, [sheet, disabled])

  const projectName = (session: Session) => projects.find(project => project.id === session.project_id)?.name || t("Проект недоступен")
  const agentName = (session: Session) => t(agents.find(agent => agent.id === session.agent)?.label || session.agent)
  const sessionCard = (session: Session, reasons = mobileAttentionReasons(session)) => {
    const summary = activitySummary(session), primary = sheet === 'attention' ? reasons[0] : undefined
    const tone = mobileSessionTone(session), label = primary?.label || reasons.find(reason => reason.kind === 'process')?.label || (tone === 'history' ? statusLabel(session.status) : summary.label)
    const showUnread = unreadResult(session) && tone !== 'result'
    const detail = toolCallLabel(session) || backgroundInventoryLabel(session)
    const description = [projectName(session), agentName(session), label, detail, `CLI: ${statusLabel(session.status)}`, primary?.detail, session.activity?.background.observed_at ? activityTime(session.activity.background.observed_at) : ''].filter(Boolean).join(' · ')
    return <button type="button" className={`mobile-session-card mobile-tone-${tone} ${selectedSessionId === session.id ? 'is-selected' : ''}`} key={session.id} disabled={disabled} onClick={() => selectSession(session)} title={`${session.title} · ${description}`} aria-label={t("{p0}, {p1}, {p2}. Открыть терминал", { p0: session.title, p1: projectName(session), p2: [label, detail, ...reasons.slice(primary ? 1 : 0).filter(reason => reason.kind !== 'result').map(reason => reason.label), showUnread ? t('Новый результат') : ''].filter((value, index, all) => value && all.indexOf(value) === index).join(' · ') })}>
      <span className="mobile-row-heading"><span className="mobile-session-signal" aria-hidden="true">{toneGlyph[tone]}</span><strong>{session.title}</strong>{showUnread && <span className="mobile-unread-result" title={t('Новый результат')} aria-label={t('Новый результат')}>●</span>}<span className="mobile-row-chevron" aria-hidden="true">›</span></span>
      <span className="mobile-card-context"><span>{projectName(session)}</span><span>{agentName(session)}</span></span>
      <span className="mobile-row-status"><span className="mobile-state"><i />{label}</span>{detail && <span className="mobile-row-detail">{detail}</span>}</span>
      {primary && <span className="mobile-action-detail">{primary.detail}</span>}
      {primary && reasons.slice(1).filter(reason => reason.kind !== 'result').map(reason => <span className="mobile-action-detail mobile-secondary-action" key={reason.kind}>{reason.label}</span>)}
    </button>
  }

  return <div ref={shell} className={`mobile-navigation ${sheet ? 'has-sheet' : ''} ${keyboardOpen ? 'keyboard-open' : ''}`} role={sheet ? 'dialog' : undefined} aria-modal={sheet ? true : undefined} aria-labelledby={sheet ? `mobile-${sheet}-title` : undefined} aria-hidden={disabled ? true : undefined} style={(dockHeight ? { '--mobile-local-dock-height': `${dockHeight}px` } : {}) as CSSProperties}>
    {sheet && <>
      <div className="mobile-sheet-backdrop" aria-hidden="true" onClick={disabled ? undefined : close} />
      <section ref={panel} className={`mobile-sheet mobile-sheet-${sheet}`} tabIndex={-1}>
        <div className="mobile-sheet-handle" aria-hidden="true" />
        <header className="mobile-sheet-heading"><div><h2 id={`mobile-${sheet}-title`}>{t(sheetTitles[sheet])}</h2><p>{sheet === 'attention' ? t("{p0} · все проекты", { p0: mobileCount(queue.length, [t("сессия"), t("сессии"), t("сессий")]) }) : sheet === 'sessions' ? scopeProject?.name || t("Все проекты") : mobileCount(projects.length, [t("проект"), t("проекта"), t("проектов")])}</p></div><LanguageSwitcher /><button className="mobile-sheet-close" type="button" disabled={disabled} aria-label={t("Закрыть панель")} onClick={close}>×</button></header>
        {sheet === 'sessions' && <div className="mobile-sheet-controls">
          {scopeProject && <div className="mobile-session-scope"><button type="button" disabled={disabled} onClick={() => { setScope(null); setFilter('all') }}>{t('← Все проекты')}</button><button type="button" className="mobile-new-session" disabled={disabled} aria-label={t('Новая сессия в проекте {p0}', { p0: scopeProject.name })} onClick={() => launch(scopeProject)}>＋</button></div>}
          <div className="mobile-session-filters" role="group" aria-label={t('Фильтр сессий')}>{filters.map(item => {
            const count = scopedSessions.filter(session => mobileSessionMatches(session, item.id)).length
            return <button type="button" key={item.id} data-filter={item.id} disabled={disabled} aria-pressed={filter === item.id} aria-label={t('{label}: {count}', { label: t(item.label), count })} onClick={() => setFilter(item.id)}><span>{t(item.label)}</span><b>{count}</b></button>
          })}</div>
        </div>}
        <div className="mobile-sheet-scroll">
          {sheet === 'attention' && <>{queue.length ? <div className="mobile-session-cards">{queue.map(entry => sessionCard(entry.session, entry.reasons))}</div> : <div className="mobile-sheet-empty"><strong>{t("Нет известных запросов внимания")}</strong><p>{t("Неизвестные состояния агента не считаются подтверждением, что все задачи завершены.")}</p><button type="button" disabled={disabled} onClick={() => onSheetChange('sessions')}>{t("Посмотреть сессии →")}</button></div>}</>}
          {sheet === 'sessions' && <>
            <div className="mobile-session-cards">{listed.map(session => sessionCard(session))}</div>
            {!listed.length && <div className="mobile-sheet-empty"><strong>{sessions.length ? t("Нет сессий для этого фильтра") : t("Сессий пока нет")}</strong><p>{filter === 'working' ? t("Здесь появляются только сессии с подтверждённым выполнением задачи агентом. Запущенного CLI-процесса недостаточно.") : filter === 'background' ? t("Здесь появляются сессии с интеграцией фоновых задач. Отсутствие интеграции не означает, что фоновых задач нет.") : t("Выберите другой фильтр или запустите новую сессию из проекта.")}</p>{filter !== 'all' && <button type="button" disabled={disabled} onClick={() => setFilter('all')}>{t("Показать все сессии")}</button>}</div>}
          </>}
          {sheet === 'projects' && <>
            <div className="mobile-project-cards">{projects.map(project => {
              const own = sessions.filter(session => session.project_id === project.id), attention = mobileAttentionEntries(own).length
              return <div className={`mobile-project-card ${projectId === project.id ? 'is-selected' : ''}`} key={project.id}><button type="button" disabled={disabled} className="mobile-project-select" onClick={() => { setScope(project.id); setFilter('all'); onSelectProject(project); onSheetChange('sessions') }}><strong>{project.name}</strong><span>{mobileCount(project.roots.length, [t("папка"), t("папки"), t("папок")])} · {mobileCount(own.length, [t("сессия"), t("сессии"), t("сессий")])}{attention ? t(" · внимание: {p0}", { p0: attention }) : ''}</span></button><button type="button" disabled={disabled} className="mobile-new-session" aria-label={t("Новая сессия в проекте {p0}", { p0: project.name })} onClick={() => launch(project)}>＋</button></div>
            })}</div>
            {!projects.length && <div className="mobile-sheet-empty"><strong>{t("Добавьте первый проект")}</strong><p>{t("Объедините локальные папки и запускайте сессии агентов.")}</p></div>}
            <button className="mobile-add-project" type="button" disabled={disabled} onClick={() => { close(); onAddProject() }}>{t("＋ Добавить проект")}</button>
            <button className="mobile-manage" type="button" disabled={disabled} onClick={() => { close(); onManage() }}>{t("⚙ Настройки и архив →")}</button>
          </>}
        </div>
      </section>
    </>}
    <div ref={dock} className="mobile-navigation-dock">
      {!keyboardOpen && <button className={`mobile-current-session mobile-tone-${current ? mobileSessionTone(current) : 'unknown'}`} type="button" disabled={disabled} onClick={event => { focusTrigger(event.currentTarget); if (current) setScope(current.project_id); setFilter('all'); onSheetChange('sessions') }}>
        <span><strong>{current?.title || t("Выберите сессию")}</strong><small>{currentProject?.name || t("Все проекты")}{current ? ` · ${activitySummary(current).label}` : ''}</small></span><span aria-hidden="true">⌃</span>
      </button>}
      <nav className="mobile-bottom-nav" aria-label={t("Основная навигация")}>
        <button type="button" disabled={disabled} aria-label={t("Внимание: {p0}", { p0: mobileCount(queue.length, [t("сессия"), t("сессии"), t("сессий")]) })} aria-pressed={sheet === 'attention'} onClick={event => { focusTrigger(event.currentTarget); onSheetChange(sheet === 'attention' ? null : 'attention') }}><span className="mobile-nav-icon" aria-hidden="true">◈{queue.length > 0 && <b>{queue.length > 99 ? '99+' : queue.length}</b>}</span><span>{t("Внимание")}</span></button>
        <button type="button" disabled={disabled} aria-pressed={sheet === 'sessions'} onClick={event => { focusTrigger(event.currentTarget); onSheetChange(sheet === 'sessions' ? null : 'sessions') }}><span className="mobile-nav-icon" aria-hidden="true">☷</span><span>{t("Сессии")}</span></button>
        <button type="button" disabled={disabled} aria-pressed={!sheet && tab === 'terminal'} onClick={event => { focusTrigger(event.currentTarget); close(); onTerminal() }}><span className="mobile-nav-icon" aria-hidden="true">›_</span><span>{t("Терминал")}</span></button>
        <button type="button" disabled={disabled} aria-pressed={sheet === 'projects'} onClick={event => { focusTrigger(event.currentTarget); onSheetChange(sheet === 'projects' ? null : 'projects') }}><span className="mobile-nav-icon" aria-hidden="true">▱</span><span>{t("Проекты")}</span></button>
      </nav>
    </div>
  </div>
}
