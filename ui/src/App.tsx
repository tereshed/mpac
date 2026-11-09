import VoiceSettingsDialog from './VoiceSettingsDialog'
import { voiceAvailable, voiceRequest } from './voice'
import type { VoiceSettings } from './voice'
import { t } from './i18n.ts'
import useLocale from './useLocale'
import LanguageSwitcher from './LanguageSwitcher'
import PhoneDialog, { desktopPhoneAvailable } from './PhoneDialog'
import TelegramSendButton from './TelegramSendButton'
import Modal from './Modal'
import { useCallback, useEffect, useRef, useState } from 'react'
import type { CSSProperties, FormEvent } from 'react'
import { ApiError, api, reconnectToDaemon, connectToDaemon, connectionErrorMessage, errorMessage, forgetRememberedToken, initialToken, isRunning, rememberVerifiedToken } from './api'
import type { LaunchDefaults, Project, Session, State } from './api'
import TerminalPane from './TerminalPane'
import FilesPane from './FilesPane'
import ReviewPane from './ReviewPane'
import ProjectEditor from './ProjectEditor'
import ProjectRow from './ProjectRow'
import ProjectSessions from './ProjectSessions'
import { ArchivedSessionsDialog, DeleteSessionDialog, RenameSessionDialog } from './SessionDialogs'
import { needsAttention, nextSessionAfterRemoval, readPreference, savePreference } from './session-management'
import type { SessionView } from './session-management'
import './session-layout.css'
import { ArchivedProjectsDialog, DeleteProjectDialog } from './ProjectDialogs'
import { reconcileProjectSelection } from './project-management'
import LaunchSession from './LaunchSession'
import LaunchDefaultsDialog from './LaunchDefaultsDialog'
import RepositoryToolbar from './RepositoryToolbar'
import { resolveTerminalFileLink } from './terminal-file-links'
import type { FileOpenRequest } from './file-navigation'
import MobileNavigation from './MobileNavigation'
import type { MobileSheet } from './MobileNavigation'
import useMobileWorkspace from './useMobileWorkspace'
import './mobile-workspace.css'
import { mobileClient, forgetMobileConnection } from './native-mobile'

type Tab = 'terminal' | 'files' | 'review'
interface ReviewLaunchRequest { rootId: string; requiredRootIds: string[]; onReady: (session: Session) => void }
interface ReviewLaunch extends ReviewLaunchRequest { project: Project; epoch: number }
const EMPTY: State = { projects: [], sessions: [], agents: [], prototype: true }


function Login({ token, initialError, onConnect }: { token: string; initialError: string; onConnect: (token: string, signal: AbortSignal) => Promise<void> }) {
  const [value, setValue] = useState(token)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState(initialError)
  const attempt = useRef<AbortController | null>(null)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; attempt.current?.abort() } }, [])
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    if (attempt.current || !value.trim()) return
    const controller = new AbortController()
    attempt.current = controller
    setBusy(true); setError('')
    try { await onConnect(value.trim(), controller.signal) }
    catch (error) { if (mounted.current && !controller.signal.aborted) setError(errorMessage(error)) }
    finally {
      if (attempt.current === controller) attempt.current = null
      if (mounted.current) setBusy(false)
    }
  }
  return <main className="login-screen"><div className="login-card"><div className="login-language"><LanguageSwitcher /></div><div className="brand-symbol large">m<span>·</span></div><span className="eyebrow">MULTI PROJECT AGENT CONTROL</span><h1>{t("Ваши агенты.")}<br />{t("Одно рабочее место.")}</h1><p>{t("Откройте актуальную ссылку запуска или вставьте её целиком в поле ниже.")}</p><form onSubmit={submit} aria-busy={busy}><label>{t("Токен или ссылка подключения")}<input type="password" value={value} onChange={e => { setValue(e.target.value); setError('') }} autoComplete="off" autoFocus required disabled={busy} aria-invalid={!!error} aria-describedby={error ? 'connection-error' : undefined} /></label>{error && <div id="connection-error" className="inline-error" role="alert">{t(error)}</div>}<button className="button primary" disabled={busy || !value.trim()}>{busy ? t("Подключаемся…") : t("Подключиться →")}</button></form><div className="local-note"><i className="tiny-dot" />{' '}{t("Данные и CLI остаются на этом компьютере.")}</div></div></main>
}

export default function App() {
  useLocale()
  const mobileViewport = useMobileWorkspace()
  const isMobile = mobileViewport.mobile
  const isTablet = mobileClient() && !isMobile
  const [tabletProjectsOpen, setTabletProjectsOpen] = useState(() => window.innerWidth >= 1100)
  const [mobileSheet, setMobileSheet] = useState<MobileSheet>(null)
  const [mobileDockHeight, setMobileDockHeight] = useState(112)
  const [repositoryDialogOpen, setRepositoryDialogOpen] = useState(false)
  const authGeneration = useRef(0)
  const [authEpoch, setAuthEpoch] = useState(0)
  const [token, setToken] = useState(initialToken)
  const [needsAuth, setNeedsAuth] = useState(false)
  const [authError, setAuthError] = useState('')
  const [state, setState] = useState<State>(EMPTY)
  const launchAgents = [...state.agents, ...(state.terminal_launches || [])]
  const [connected, setConnected] = useState(false)
  const [connectionError, setConnectionError] = useState('')
  const [projectId, setProjectId] = useState(() => readPreference('mpac-selected-project', ''))
  const [sessionId, setSessionId] = useState('')
  const [rootId, setRootId] = useState('')
  const [tab, setTab] = useState<Tab>('terminal')
  const [voice, setVoice] = useState<VoiceSettings | null>(null)
  const [modal, setModal] = useState<'project' | 'session' | 'delete_project' | 'archived_projects' | 'delete_session' | 'archived_sessions' | 'rename_session' | 'launch_defaults' | 'voice_settings' | null>(null)
  const [phoneDialog, setPhoneDialog] = useState(false)
  const [workspaceToolsOpen, setWorkspaceToolsOpen] = useState(false)
  const [modalProject, setModalProject] = useState<Project | null>(null)
  const [defaultsProject, setDefaultsProject] = useState<Project | undefined>(undefined)
  const [reviewLaunch, setReviewLaunch] = useState<ReviewLaunch | null>(null)
  const [requestedFile, setRequestedFile] = useState<FileOpenRequest | null>(null)
  const [repositoryRevision, setRepositoryRevision] = useState(0)
  const [repositoryBusy, setRepositoryBusy] = useState(false)
  const [modalSession, setModalSession] = useState<Session | null>(null)
  const [lastArchivedSession, setLastArchivedSession] = useState<Session | null>(null)
  const [restoringSessionId, setRestoringSessionId] = useState<string | null>(null)
  const restoringSession = useRef(false)
  const [expandedProjects, setExpandedProjects] = useState<Record<string, boolean>>({})
  const [sessionView, setSessionView] = useState<SessionView>(() => readPreference('mpac-session-view', 'list') === 'dots' ? 'dots' : 'list')
  const [lastArchivedProject, setLastArchivedProject] = useState<Project | null>(null)
  const [restoringProjectId, setRestoringProjectId] = useState<string | null>(null)
  const restoring = useRef(false)
  const projectRevision = useRef(0)
  const [showProjects, setShowProjects] = useState(false)
  const [attentionOnly, setAttentionOnly] = useState(false)
  const [actionError, setActionError] = useState('')
  const [actionNotice, setActionNotice] = useState('')
  const [stopping, setStopping] = useState(false)
  const [, tick] = useState(0)
  useEffect(() => { if (!isMobile) { setMobileSheet(null); setShowProjects(false) } }, [isMobile])
  useEffect(() => { setMobileSheet(null); setShowProjects(false); setWorkspaceToolsOpen(false) }, [authEpoch])
  useEffect(() => { if (modal || reviewLaunch || showProjects) setMobileSheet(null) }, [modal, reviewLaunch, showProjects])

  useEffect(() => {
    const receiveToken = () => {
      const supplied = new URLSearchParams(location.hash.slice(1)).get('token')
      if (!supplied?.trim()) return
      authGeneration.current += 1
      setAuthEpoch(authGeneration.current)
      setToken(initialToken())
      setNeedsAuth(false)
      setAuthError('')
    }
    window.addEventListener('hashchange', receiveToken)
    return () => window.removeEventListener('hashchange', receiveToken)
  }, [])

  const refresh = useCallback(async (signal?: AbortSignal) => {
    const generation = authEpoch
    const revision = projectRevision.current
    if (generation !== authGeneration.current) return
    const verified = await reconnectToDaemon(token, signal)
    if (signal?.aborted || generation !== authGeneration.current || revision !== projectRevision.current) return
    rememberVerifiedToken(verified.token)
    if (verified.token !== token) {
      // In-flight refreshes using the replaced credential must not commit.
      authGeneration.current += 1
      setAuthEpoch(authGeneration.current)
      setToken(verified.token)
    }
    setState(verified.state); setConnected(true); setConnectionError('')
  }, [token, authEpoch])

  useEffect(() => {
    if (!token || needsAuth) return
    let active = true
    const abort = new AbortController()
    let timer: ReturnType<typeof setTimeout>
    const generation = authEpoch
    const poll = async () => {
      if (!active || generation !== authGeneration.current) return
      try { await refresh(abort.signal) }
      catch (e) {
        if (!active || generation !== authGeneration.current) return
        setConnected(false)
        if (e instanceof ApiError && (e.status === 401 || e.status === 403)) { setAuthError(connectionErrorMessage(e)); setNeedsAuth(true) }
        else setConnectionError(errorMessage(e))
      }
      if (active) timer = setTimeout(poll, 1500)
    }
    void poll()
    const clock = setInterval(() => tick(v => v + 1), 1000)
    return () => { active = false; abort.abort(); clearTimeout(timer); clearInterval(clock) }
  }, [token, needsAuth, refresh, authEpoch])

  const project = state.projects.find(p => p.id === projectId) || state.projects[0]
  const sessions = state.sessions.filter(s => s.project_id === project?.id)
  const selectedSession = sessions.find(s => s.id === sessionId) || (sessionId ? undefined : sessions.find(isRunning) || sessions[0])
  const root = project?.roots.find(r => r.id === rootId) || project?.roots.find(r => r.id === selectedSession?.root_id) || project?.roots[0]
  const visibleSessions = state.sessions.filter(session => state.projects.some(project => project.id === session.project_id))
  const attention = visibleSessions.filter(needsAttention)
  const live = visibleSessions.filter(isRunning)
  const managementAvailable = state.capabilities?.project_management === true
  const archivedProjects = state.archived_projects || []
  const sessionManagementAvailable = state.capabilities?.session_management === true
  const archivedSessions = state.archived_sessions || []
  const sessionRenameAvailable = state.capabilities?.session_rename === true
  const launchDefaultsAvailable = state.capabilities?.launch_defaults === true
  const sharedRepositoryAvailable = state.capabilities?.git_commits === true
  const commentsAvailable = state.capabilities?.review_comments === true
  const repositoryContext = authEpoch + ':' + (project?.id || '') + ':' + (root?.id || '')
  const currentRepositoryContext = useRef(repositoryContext)
  currentRepositoryContext.current = repositoryContext
  useEffect(() => { setRepositoryBusy(false); setRepositoryRevision(value => value + 1); setReviewLaunch(null) }, [repositoryContext])
  const repositoryChanged = () => { if (currentRepositoryContext.current === repositoryContext) setRepositoryRevision(value => value + 1) }
  const beforeRepositoryMutation = () => { if (currentRepositoryContext.current === repositoryContext) { setRepositoryBusy(true); setRepositoryRevision(value => value + 1) } }
  const afterRepositoryMutation = () => { if (currentRepositoryContext.current === repositoryContext) { setRepositoryBusy(false); setRepositoryRevision(value => value + 1) } }
  const openLaunchForReview = (request: ReviewLaunchRequest) => {
    if (!project || !commentsAvailable || authEpoch !== authGeneration.current) return
    setRootId(root?.id || request.rootId)
    setReviewLaunch({ ...request, project, epoch: authEpoch })
  }
  const openTerminalFile = (reference: string) => {
    if (!project || !selectedSession || authEpoch !== authGeneration.current) return
    if (repositoryBusy || modal || reviewLaunch) return
    const mainRoot = project.roots.find(item => item.id === selectedSession.root_id)
    if (!mainRoot) { setActionError(t("Рабочая папка этой сессии удалена из проекта. Добавьте её в проект, чтобы открывать ссылки на файлы.")); return }
    try {
      // A CLI-relative path belongs to its launch folder, regardless of the
      // repository currently selected in the shared Git toolbar.
      const target = resolveTerminalFileLink(reference, mainRoot, project.roots)
      setRequestedFile({ ...target, requestId: crypto.randomUUID(), epoch: authEpoch, projectId: project.id })
      setRootId(target.rootId); setTab('files'); setShowProjects(false); setActionError('')
    } catch (failure) { setActionError(errorMessage(failure)) }
  }
  const markActivityRead = async (expectedRevision: number) => {
    if (!selectedSession || authEpoch !== authGeneration.current) throw new Error(t("Сессия изменилась. Откройте результат заново."))
    const generation = authGeneration.current, id = selectedSession.id
    const saved = await api<Session>(token, `/sessions/${id}/activity/read`, { expected_revision: expectedRevision })
    if (generation !== authGeneration.current || saved.id !== id) throw new Error(t("Подключение изменилось. Обновите состояние сессии."))
    projectRevision.current += 1
    setState(previous => ({ ...previous, sessions: previous.sessions.map(item => {
      if (item.id !== id || !saved.activity) return item
      if (!item.activity) return { ...item, activity: saved.activity }
      // A newer native event may have arrived while this acknowledgement was
      // in flight. Merge only the read receipt, retaining the live phase/jobs.
      return { ...item, activity: { ...item.activity, result: { ...item.activity.result,
        read_revision: Math.min(item.activity.result.revision, Math.max(item.activity.result.read_revision, saved.activity.result.read_revision)),
      } } }
    }) }))
    refreshSafely()
  }

  const chooseProject = (id: string) => { setRequestedFile(null); setProjectId(id); savePreference('mpac-selected-project', id); setExpandedProjects(previous => ({ ...previous, [id]: true })); setSessionId(''); setRootId(''); setActionError('') }
  const chooseSession = (session: Session) => { setMobileSheet(null); setRequestedFile(null); setProjectId(session.project_id); savePreference('mpac-selected-project', session.project_id); setExpandedProjects(previous => ({ ...previous, [session.project_id]: true })); setSessionId(session.id); setRootId(session.root_id); setTab('terminal'); setShowProjects(false); setActionError('') }
  const refreshSafely = () => { void refresh().catch(e => setActionError(errorMessage(e))) }
  useEffect(() => {
    setVoice(null)
    if (!token || !voiceAvailable()) return
    let active = true
    void voiceRequest<VoiceSettings>(token, 'settings').then(
      settings => { if (active) setVoice(settings) },
      () => { /* The settings dialog shows storage errors on demand. */ },
    )
    return () => { active = false }
  }, [token, authEpoch])

  const openProjectEditor = (editing?: Project) => { setModalProject(editing || null); setShowProjects(false); setModal('project') }
  const openDeleteProject = (deleting: Project) => { if (!managementAvailable) return; setModalProject(deleting); setShowProjects(false); setModal('delete_project') }
  const openLaunchDefaults = (target?: Project) => { if (!launchDefaultsAvailable) return; setDefaultsProject(target); setShowProjects(false); setModal('launch_defaults') }
  const launchDefaultsSaved = (defaults: LaunchDefaults) => {
    if (authEpoch !== authGeneration.current) return false
    projectRevision.current += 1
    setState(previous => ({ ...previous, launch_defaults: defaults }))
    setActionNotice(t("Настройки запуска сохранены.")); refreshSafely(); return true
  }
  const openLaunch = () => { if (project) setModal('session'); else openProjectEditor() }
  const launchInProject = (selected: Project) => { chooseProject(selected.id); setShowProjects(false); setModal('session') }
  const openDeleteSession = (session: Session) => { if (!sessionManagementAvailable) return; setModalSession(session); setShowProjects(false); setModal('delete_session') }
  const openRenameSession = (session: Session) => { if (!sessionRenameAvailable) return; setModalSession(session); setShowProjects(false); setModal('rename_session') }
  const sessionRenamed = (saved: Session) => {
    if (authEpoch !== authGeneration.current) return false
    projectRevision.current += 1
    setState(previous => ({ ...previous, sessions: previous.sessions.map(item => item.id === saved.id ? saved : item) }))
    setModal(null); setModalSession(null); setActionNotice(t("Название сессии сохранено.")); refreshSafely(); return true
  }
  const sessionDeleted = (deleted: Session) => {
    if (authEpoch !== authGeneration.current) return false
    projectRevision.current += 1
    setState(previous => ({ ...previous, sessions: previous.sessions.filter(item => item.id !== deleted.id), archived_sessions: [...(previous.archived_sessions || []).filter(item => item.id !== deleted.id), deleted] }))
    if (selectedSession?.id === deleted.id) {
      const next = nextSessionAfterRemoval(state.sessions, deleted.project_id, deleted.id)
      setSessionId(next?.id || ''); setRootId(next?.root_id || project?.roots[0]?.id || '')
    }
    setLastArchivedSession(deleted); setModal(null); setModalSession(null); refreshSafely(); return true
  }
  const restoreSession = async (archived: Session) => {
    if (!sessionManagementAvailable || restoringSession.current) throw new Error(t("Дождитесь завершения текущей операции."))
    const generation = authGeneration.current
    restoringSession.current = true; setRestoringSessionId(archived.id)
    try {
      const result = await api<{session: Session}>(token, `/sessions/${archived.id}/restore`, {})
      const saved = result.session
      if (!saved || saved.id !== archived.id || isRunning(saved)) throw new Error(t("Неожиданный ответ восстановления. Обновите список сессий."))
      if (generation !== authGeneration.current) throw new Error(t("Подключение изменилось. Обновите список сессий."))
      projectRevision.current += 1
      setState(previous => ({ ...previous, sessions: previous.sessions.some(item => item.id === saved.id) ? previous.sessions.map(item => item.id === saved.id ? saved : item) : [...previous.sessions, saved], archived_sessions: (previous.archived_sessions || []).filter(item => item.id !== saved.id) }))
      setLastArchivedSession(previous => previous?.id === archived.id ? null : previous)
      chooseSession(saved); setActionNotice(t("Сессия «{p0}» восстановлена в списке.", { p0: saved.title })); refreshSafely()
    } finally { restoringSession.current = false; setRestoringSessionId(null) }
  }
  const projectSaved = (saved: Project) => {
    if (authEpoch !== authGeneration.current) return false
    projectRevision.current += 1
    setState(previous => ({ ...previous, projects: previous.projects.some(item => item.id === saved.id) ? previous.projects.map(item => item.id === saved.id ? saved : item) : [...previous.projects, saved] }))
    if (modalProject) {
      if (project?.id === saved.id) {
        const selection = reconcileProjectSelection(saved, root?.id || '', selectedSession?.id || '', state.sessions)
        setRootId(selection.rootId); setSessionId(selection.sessionId)
      }
      setActionNotice(t("Изменения проекта сохранены."))
    } else chooseProject(saved.id)
    setModal(null); setModalProject(null); refreshSafely(); return true
  }
  const projectDeleted = (deleted: Project) => {
    if (authEpoch !== authGeneration.current) return false
    projectRevision.current += 1
    setState(previous => ({ ...previous, projects: previous.projects.filter(item => item.id !== deleted.id), archived_projects: [...(previous.archived_projects || []).filter(item => item.id !== deleted.id), deleted] }))
    if (project?.id === deleted.id) chooseProject(state.projects.find(item => item.id !== deleted.id)?.id || '')
    setLastArchivedProject(deleted); setModal(null); setModalProject(null); setActionNotice(''); refreshSafely(); return true
  }
  const restoreProject = async (archived: Project): Promise<Project> => {
    if (!managementAvailable) throw new Error(t("Восстановление пока недоступно в запущенном daemon."))
    if (restoring.current) throw new Error(t("Дождитесь завершения восстановления проекта."))
    const generation = authGeneration.current
    restoring.current = true; setRestoringProjectId(archived.id); setActionError('')
    try {
      const saved = await api<Project>(token, `/projects/${archived.id}/restore`, {})
      if (generation !== authGeneration.current) throw new Error(t("Подключение изменилось. Обновите список проектов."))
      projectRevision.current += 1
      setState(previous => ({ ...previous, projects: previous.projects.some(item => item.id === saved.id) ? previous.projects.map(item => item.id === saved.id ? saved : item) : [...previous.projects, saved], archived_projects: (previous.archived_projects || []).filter(item => item.id !== saved.id) }))
      chooseProject(saved.id); setLastArchivedProject(previous => previous?.id === saved.id ? null : previous); setActionNotice(t("Проект «{p0}» восстановлен.", { p0: saved.name })); refreshSafely()
      return saved
    } finally { restoring.current = false; setRestoringProjectId(null) }
  }

  const connect = async (next: string, signal: AbortSignal) => {
    const generation = ++authGeneration.current
    const verified = await connectToDaemon(next, signal)
    if (signal.aborted || generation !== authGeneration.current) return
    rememberVerifiedToken(verified.token)
    setAuthEpoch(generation)
    setToken(verified.token); setState(verified.state); setConnected(true)
    setNeedsAuth(false); setAuthError(''); setConnectionError('')
  }
  const disconnect = async () => {
    if (mobileClient()) {
      try { await forgetMobileConnection() }
      catch { setActionError(t('Не удалось удалить сохранённое подключение.')); return }
    }
    authGeneration.current += 1
    setAuthEpoch(authGeneration.current)
    forgetRememberedToken(); setToken(''); setState(EMPTY)
    setConnected(false); setAuthError('')
  }
  const stop = async () => {
    if (!selectedSession || stopping) return
    setStopping(true); setActionError('')
    try { await api(token, `/sessions/${selectedSession.id}/stop`, {}); await refresh() }
    catch (e) { setActionError(errorMessage(e)) }
    finally { setStopping(false) }
  }

  if (!token || needsAuth) return <Login token={token} initialError={authError} onConnect={connect} />

  return <div className={`app-shell session-workspace ${isMobile ? 'mobile-workspace' : ''} ${isTablet ? 'tablet-workspace' : ''} ${isTablet && tabletProjectsOpen ? 'tablet-projects-open' : ''} ${mobileViewport.keyboardOpen ? 'mobile-keyboard' : ''}`} style={isMobile || mobileClient() ? { '--mobile-viewport-height': `${mobileViewport.height}px`, '--mobile-viewport-top': `${mobileViewport.top}px`, '--mobile-viewport-bottom': `${mobileViewport.bottom}px`, '--mobile-dock-height': `${mobileDockHeight}px` } as CSSProperties : undefined}>
    <aside id="workspace-projects" className={`project-rail ${showProjects ? 'mobile-open' : ''}`} inert={isMobile && !!mobileSheet}>
      <a className="brand" href="/" onClick={e => e.preventDefault()} aria-label="MPAC"><div className="brand-symbol">m<span>·</span></div><div><strong>MPAC</strong><span>{t("Рабочее место агентов")}</span></div></a>
      <div className="rail-heading"><span>{t("ПРОЕКТЫ")}</span><button className="icon-button" aria-label={t("Добавить проект")} title={t("Добавить проект")} onClick={() => openProjectEditor()}>+</button></div>
      <div className="rail-session-controls"><div className="session-view-toggle" role="group" aria-label={t("Вид сессий")}><button type="button" aria-pressed={sessionView === 'list'} onClick={() => { setSessionView('list'); savePreference('mpac-session-view', 'list') }} title={t("Список сессий")}>{t("☷ Список")}</button><button type="button" aria-pressed={sessionView === 'dots'} onClick={() => { setSessionView('dots'); savePreference('mpac-session-view', 'dots') }} title={t("Компактные кружки")}>{t("⠿ Кружки")}</button></div><button type="button" className={`rail-attention-filter ${attentionOnly ? 'active' : ''}`} aria-pressed={attentionOnly} title={t("Показать только запросы внимания и ошибки")} onClick={() => setAttentionOnly(!attentionOnly)}>{t("◈ Внимание")}</button></div>
      <div className="session-status-legend" aria-label={t("Цвета статусов сессий")}><span title={t("CLI-процесс работает")}><i className="status-dot status-running" />{t("Запущена")}</span><span title={t("Нужен ответ либо произошла ошибка")}><i className="status-dot status-needs_attention" />{t("Внимание")}</span><span title={t("Процесс завершён или остановлен")}><i className="status-dot status-stopped" />{t("Завершена")}</span></div>
      <nav className="project-list" aria-label={t("Проекты и сессии")}>{state.projects.map((p, i) => {
        const projectSessions = state.sessions.filter(s => s.project_id === p.id)
        const expanded = expandedProjects[p.id] ?? p.id === project?.id
        return <div className="project-group" key={p.id}><ProjectRow project={p} index={i} selected={p.id === project?.id} runningCount={projectSessions.filter(isRunning).length} sessionCount={projectSessions.length} expanded={expanded} onToggle={() => setExpandedProjects(previous => ({ ...previous, [p.id]: !expanded }))} canManage={managementAvailable} onSelect={() => chooseProject(p.id)} onEdit={() => openProjectEditor(p)} onDelete={() => openDeleteProject(p)} onLaunchDefaults={launchDefaultsAvailable ? () => openLaunchDefaults(p) : undefined} />{expanded && <ProjectSessions project={p} sessions={projectSessions} agents={launchAgents} selectedId={selectedSession?.id || ''} view={sessionView} attentionOnly={attentionOnly} canManage={sessionManagementAvailable} onSelect={chooseSession} onDelete={openDeleteSession} onLaunch={() => launchInProject(p)} />}</div>
      })}</nav>
      <button className="add-project" onClick={() => openProjectEditor()}>{t("＋ Добавить проект")}</button>
      {isMobile && actionNotice && <div className="mobile-menu-notice" role="status"><span>{t(actionNotice)}</span><button type="button" className="icon-button" onClick={() => setActionNotice('')} aria-label={t('Закрыть уведомление')}>×</button></div>}
      {isMobile && showProjects && <button type="button" className="mobile-rail-close" onClick={() => setShowProjects(false)}>{t("Готово · к рабочему месту")}</button>}
      <div className="rail-bottom"><button type="button" className="workspace-tools-trigger" onClick={() => { setShowProjects(false); setMobileSheet(null); setWorkspaceToolsOpen(true) }}><span aria-hidden="true">⚙</span><span>{t('Настройки и подключения')}</span><i className={`tiny-dot ${connected ? '' : 'offline'}`} aria-label={connected ? t('Подключено') : t('Нет соединения')} /></button></div>
    </aside>
    {showProjects && <div className="mobile-scrim" onClick={() => setShowProjects(false)} />}
    <main className="workspace" inert={isMobile && (!!mobileSheet || showProjects)}>
      <header className="workspace-header"><div className="header-title">{isTablet && <button type="button" className="tablet-projects-toggle" aria-label={t(tabletProjectsOpen ? 'Скрыть проекты' : 'Показать проекты')} aria-expanded={tabletProjectsOpen} aria-controls="workspace-projects" onClick={() => setTabletProjectsOpen(value => !value)}>☰</button>}<button className="icon-button mobile-menu" onClick={() => setShowProjects(!showProjects)} aria-label={t("Проекты")}>☰</button><span className="breadcrumb">{t("Рабочее пространство")}{' '}<span>/</span></span><h1>{project?.name || t("Ваши проекты")}</h1><span className="prototype-badge">LOCAL</span></div><div className="header-actions"><div className="header-session-summary" aria-label={t("Сводка сессий")}><span className="overview-inline-stat" title={t("CLI-процесс работает")}><i className="status-dot status-running" /><strong>{live.length}</strong>{' '}{t("запущено")}</span><button type="button" className={`overview-inline-stat overview-attention ${attentionOnly ? 'active' : ''}`} aria-pressed={attentionOnly} onClick={() => { if (isMobile) { setMobileSheet('attention'); return }; setAttentionOnly(!attentionOnly); if (!attentionOnly) setExpandedProjects(Object.fromEntries(state.projects.filter(p => attention.some(s => s.project_id === p.id)).map(p => [p.id, true]))) }}><i className="status-dot status-needs_attention" /><strong>{attention.length}</strong>{' '}{t(attention.length === 1 ? "требует внимания" : "требуют внимания")}</button></div><LanguageSwitcher /><span className="connection"><i className={`tiny-dot ${connected ? '' : 'offline'}`} />{connected ? t("Подключено") : t("Переподключение…")}</span><button className="button primary" aria-label={project ? t("Новая сессия") : t("Добавить проект")} onClick={openLaunch}>＋ <span>{project ? t("Новая сессия") : t("Добавить проект")}</span></button></div></header>
      {connectionError && <div className="connection-banner" role="status">{t("Нет связи с daemon: {p0}. Повторяем подключение…", {p0: t(connectionError)})}</div>}

      {!isMobile && lastArchivedProject && <div className="project-undo-banner" role="status"><span>{t("Проект «{p0}» удалён из приложения. Файлы и история сохранены.", {p0: lastArchivedProject.name})}</span>{managementAvailable && <button className="text-button" disabled={!!restoringProjectId} onClick={() => { void restoreProject(lastArchivedProject).catch(failure => setActionError(errorMessage(failure))) }}>{restoringProjectId === lastArchivedProject.id ? t("Восстанавливаем…") : t("Восстановить")}</button>}<button className="icon-button" aria-label={t("Закрыть уведомление об удалении")} disabled={!!restoringProjectId} onClick={() => setLastArchivedProject(null)}>×</button></div>}
      {!isMobile && lastArchivedSession && <div className="project-undo-banner" role="status"><span>{t("Сессия «{p0}» удалена из списка.", {p0: lastArchivedSession.title})}</span><button type="button" className="text-button" disabled={!!restoringSessionId || !sessionManagementAvailable} onClick={() => { void restoreSession(lastArchivedSession).catch(failure => setActionError(errorMessage(failure))) }}>{t("Восстановить")}</button><button type="button" className="icon-button" aria-label={t("Закрыть уведомление об удалении сессии")} onClick={() => setLastArchivedSession(null)}>×</button></div>}
      {actionError && <div className="action-error inline-error" role="alert"><span>{t(actionError)}</span><button className="icon-button" onClick={() => setActionError('')} aria-label={t("Закрыть ошибку")}>×</button></div>}
      {!isMobile && actionNotice && <div className="action-error success-notice" role="status"><span>{t(actionNotice)}</span><button className="icon-button" onClick={() => setActionNotice('')} aria-label={t("Закрыть уведомление")}>×</button></div>}
      <div className="workbench">

        <section className="main-panel">{sharedRepositoryAvailable && project && root && <RepositoryToolbar key={repositoryContext} token={token} project={project} root={root} onRoot={setRootId} revision={repositoryRevision} branchesAvailable={state.capabilities?.git_branches === true} commitsAvailable={state.capabilities?.git_commits === true} busy={repositoryBusy} onBeforeMutation={beforeRepositoryMutation} onAfterMutation={afterRepositoryMutation} onExternalChange={repositoryChanged} onDialogChange={setRepositoryDialogOpen} compact={isMobile} />}<div className="panel-tabs"><button className={tab === 'terminal' ? 'active' : ''} onClick={() => setTab('terminal')}><span>▰</span>{' '}{t("Терминал")}</button><button className={tab === 'files' ? 'active' : ''} onClick={() => setTab('files')}><span>▱</span>{' '}{t("Файлы")}</button><button className={tab === 'review' ? 'active' : ''} onClick={() => setTab('review')}><span>⑂</span>{' '}{t("Изменения")}</button><div className="panel-tabs-spacer" />{tab === 'terminal' && selectedSession && <span className="tab-session-name">{selectedSession.title}</span>}{tab === 'terminal' && selectedSession && sessionRenameAvailable && <button type="button" className="session-panel-rename" title={t("Изменить название сессии")} aria-label={t("Переименовать сессию {p0}", { p0: selectedSession.title })} onClick={() => openRenameSession(selectedSession)}>✎</button>}{tab === 'terminal' && selectedSession && sessionManagementAvailable && <button type="button" className="session-panel-delete" title={t("Удалить сессию из списка")} aria-label={t("Удалить сессию {p0}", { p0: selectedSession.title })} onClick={() => openDeleteSession(selectedSession)}>⌫</button>}{tab === 'terminal' && selectedSession && isRunning(selectedSession) && <button className="stop-button" aria-label={t("Остановить сессию")} disabled={stopping} onClick={stop}>{stopping ? '…' : '■'} <span>{t("Остановить")}</span></button>}</div>
          {sharedRepositoryAvailable && tab === 'terminal' && selectedSession && root && root.id !== selectedSession.root_id && <div className="notice">{t("Git-панель: {p0}. Терминал работает в папке {p1}.", {p0: root.name, p1: project?.roots.find(item => item.id === selectedSession.root_id)?.name || t("выбранной сессии")})}{' '}<button className="text-button" disabled={repositoryBusy || !project?.roots.some(item => item.id === selectedSession.root_id)} onClick={() => setRootId(selectedSession.root_id)}>{t("К папке сессии")}</button></div>}
          <div className={`tab-content ${tab === 'terminal' ? '' : 'hidden'}`}>{selectedSession ? <>{selectedSession.error && <div className="inline-error">{t(selectedSession.error)}</div>}<TerminalPane token={token} session={selectedSession} voice={voice} compact={isMobile} autoFocus={!isMobile} visible={tab === 'terminal' && !modal && !reviewLaunch && !mobileSheet && !(isMobile && showProjects) && !repositoryDialogOpen && !workspaceToolsOpen && !phoneDialog} attachmentsAvailable={state.capabilities?.session_attachments === true} streamingAvailable={state.capabilities?.terminal_stream === true} answersAvailable={state.capabilities?.agent_answers === true} resizeAvailable={state.capabilities?.terminal_resize !== false} onOpenFile={openTerminalFile} onMarkActivityRead={markActivityRead} /></> : <div className="empty-state welcome"><div className="welcome-graphic"><span>⌘</span><i /><span>▰</span></div><span className="eyebrow">{t("РАБОТА НАЧИНАЕТСЯ ЗДЕСЬ")}</span><h2>{t("Дайте каждой задаче свою сессию")}</h2><p>{t("Запускайте CLI внутри проекта, следите за выводом")}<br className="desktop-break" />{' '}{t("и возвращайтесь к работе из одного окна.")}</p><button className="button primary" onClick={openLaunch}>{project ? t("Попробовать демо →") : t("Добавить первый проект →")}</button><div className="welcome-features"><span>{t("◉ Локальный запуск")}</span><span>M↓ Markdown</span><span>{t("⑂ Ревью изменений")}</span></div></div>}</div>
          {tab === 'files' && project && root && <div className="tab-content"><FilesPane key={`${authEpoch}:${project.id}:${root.id}`} token={token} project={project} root={root} onRoot={setRootId} branchesAvailable={state.capabilities?.git_branches === true} externalToolbar={sharedRepositoryAvailable} repositoryRevision={repositoryRevision} repositoryBusy={repositoryBusy} commentsAvailable={commentsAvailable} sessions={sessions} onLaunchForReview={openLaunchForReview} onOpenSession={chooseSession} requestedFile={requestedFile?.epoch === authEpoch && requestedFile.projectId === project.id ? requestedFile : null} onFileOpened={requestId => setRequestedFile(previous => previous?.requestId === requestId ? null : previous)} /></div>}
          {tab === 'review' && project && root && <div className="tab-content"><ReviewPane key={`${authEpoch}:${project.id}:${root.id}`} token={token} project={project} root={root} session={selectedSession} onRoot={setRootId} externalToolbar={sharedRepositoryAvailable} repositoryRevision={repositoryRevision} repositoryBusy={repositoryBusy} commentsAvailable={commentsAvailable} sessions={sessions} onLaunchForReview={openLaunchForReview} onOpenSession={chooseSession} onSent={() => { setActionNotice(t("Черновик вставлен в терминал. Проверьте текст и нажмите Enter.")); setTab('terminal'); refreshSafely() }} /></div>}
          {(tab === 'files' || tab === 'review') && !project && <div className="empty-state"><p>{t("Добавьте проект, чтобы читать файлы и смотреть изменения.")}</p></div>}
        </section>
      </div>
      <footer className="workspace-footer"><span><i className="tiny-dot" /> LOCAL FIRST</span><span>{t("Rust daemon · CLI авторизация · без автоматического API fallback")}</span><span>{t("Прототип 0.1")}</span></footer>
    </main>
    {isMobile && <MobileNavigation key={`mobile:${authEpoch}`} projects={state.projects} sessions={visibleSessions} agents={launchAgents} projectId={project?.id || ''} selectedSessionId={selectedSession?.id || ''} tab={tab} sheet={mobileSheet} onSheetChange={setMobileSheet} onSelectSession={chooseSession} onSelectProject={selected => chooseProject(selected.id)} onTerminal={() => { setMobileSheet(null); setShowProjects(false); setTab('terminal') }} onLaunch={selected => { setMobileSheet(null); launchInProject(selected) }} onAddProject={() => { setMobileSheet(null); openProjectEditor() }} onManage={() => { setMobileSheet(null); setShowProjects(false); setWorkspaceToolsOpen(true) }} disabled={!!modal || !!reviewLaunch || showProjects || repositoryDialogOpen || workspaceToolsOpen || phoneDialog} keyboardOpen={mobileViewport.keyboardOpen} onDockHeight={setMobileDockHeight} />}
    {modal === 'project' && <ProjectEditor key={modalProject?.id || 'create'} token={token} expected={modalProject || undefined} sessions={state.sessions} managementAvailable={managementAvailable} onClose={() => { setModal(null); setModalProject(null) }} onSaved={projectSaved} />}
    {modal === 'delete_project' && modalProject && <DeleteProjectDialog token={token} project={modalProject} sessions={state.sessions} enabled={managementAvailable} onClose={() => { setModal(null); setModalProject(null) }} onDeleted={projectDeleted} />}
    {modal === 'archived_projects' && <ArchivedProjectsDialog projects={archivedProjects} enabled={managementAvailable} restoringId={restoringProjectId} onClose={() => setModal(null)} onRestore={restoreProject} />}
    {modal === 'rename_session' && modalSession && <RenameSessionDialog key={modalSession.id} token={token} session={modalSession} sessions={state.sessions} enabled={sessionRenameAvailable} onClose={() => { setModal(null); setModalSession(null) }} onSaved={sessionRenamed} />}
    {modal === 'delete_session' && modalSession && <DeleteSessionDialog key={modalSession.id} token={token} session={modalSession} sessions={state.sessions} enabled={sessionManagementAvailable} onClose={() => { setModal(null); setModalSession(null) }} onDeleted={sessionDeleted} onStopped={async () => { await refresh() }} />}
    {modal === 'archived_sessions' && <ArchivedSessionsDialog sessions={archivedSessions} projects={state.projects} restoringId={restoringSessionId} onClose={() => setModal(null)} onRestore={restoreSession} />}
    {workspaceToolsOpen && <Modal title={t('Настройки и подключения')} subtitle={t('Архивы, параметры агентов и подключение устройств')} onClose={() => setWorkspaceToolsOpen(false)}><div className="workspace-tools-list">
      {managementAvailable && <button className="archived-projects-trigger" onClick={() => { setShowProjects(false); setWorkspaceToolsOpen(false); setModal('archived_projects') }}>{t("▱ Удалённые проекты")}{' '}<span>{archivedProjects.length}</span></button>}
      {sessionManagementAvailable && <button className="archived-projects-trigger archived-sessions-trigger" onClick={() => { setShowProjects(false); setWorkspaceToolsOpen(false); setModal('archived_sessions') }}>{t("◷ Удалённые сессии")}{' '}<span>{archivedSessions.length}</span></button>}
      {launchDefaultsAvailable && <button className="archived-projects-trigger launch-defaults-trigger" onClick={() => { setWorkspaceToolsOpen(false); openLaunchDefaults() }}>{t("⚙ Настройки запуска")}</button>}
      {voiceAvailable() && <button type="button" className="archived-projects-trigger" onClick={() => { setWorkspaceToolsOpen(false); setModal('voice_settings') }}>{t('Голосовой ввод')}</button>}
      {desktopPhoneAvailable() && <button className="archived-projects-trigger" onClick={() => { setWorkspaceToolsOpen(false); setPhoneDialog(true) }}>{t('Телефон и Telegram')}</button>}
      {desktopPhoneAvailable() && <TelegramSendButton key={authEpoch} token={token} onConfigure={() => { setWorkspaceToolsOpen(false); setPhoneDialog(true) }} />}
      <button type="button" className="archived-projects-trigger" onClick={refreshSafely}>{t('Обновить сессии')}</button>
      </div><div className="workspace-tools-footer"><span><i className={`tiny-dot ${connected ? '' : 'offline'}`} /> {connected ? t('Подключено') : t('Нет соединения')} · MPAC 0.1</span><button type="button" className="button secondary compact" onClick={() => { setWorkspaceToolsOpen(false); void disconnect() }}>{t('Отключиться')}</button></div></Modal>}
    {modal === 'voice_settings' && <VoiceSettingsDialog key={authEpoch} token={token} onClose={() => setModal(null)} onSaved={setVoice} />}
    {phoneDialog && <PhoneDialog key={authEpoch} token={token} onClose={() => setPhoneDialog(false)} />}
    {modal === 'launch_defaults' && state.launch_defaults && <LaunchDefaultsDialog key={`${authEpoch}:${defaultsProject?.id || 'global'}`} token={token} project={defaultsProject} agents={launchAgents} defaults={state.launch_defaults} onClose={() => setModal(null)} onSaved={launchDefaultsSaved} />}
    {reviewLaunch && reviewLaunch.epoch === authEpoch && <LaunchSession key={`review:${authEpoch}:${reviewLaunch.project.id}`} token={token} project={reviewLaunch.project} agents={launchAgents} rootId={reviewLaunch.rootId} initialRootId={reviewLaunch.rootId} requiredRootIds={reviewLaunch.requiredRootIds} defaults={state.launch_defaults} environmentFilesAvailable={state.capabilities?.session_env_files === true} autoTitleAvailable={state.capabilities?.auto_session_titles === true} onClose={() => setReviewLaunch(null)} onLaunched={saved => { if (authEpoch !== authGeneration.current || currentRepositoryContext.current !== repositoryContext) return; projectRevision.current += 1; setState(previous => ({ ...previous, sessions: [saved, ...previous.sessions.filter(item => item.id !== saved.id)] })); setReviewLaunch(null); reviewLaunch.onReady(saved); refreshSafely() }} />}
    {modal === 'session' && project && <LaunchSession key={authEpoch} token={token} project={project} agents={launchAgents} rootId={root?.id || ''} defaults={state.launch_defaults} environmentFilesAvailable={state.capabilities?.session_env_files === true} autoTitleAvailable={state.capabilities?.auto_session_titles === true} onClose={() => setModal(null)} onLaunched={s => { if (authEpoch !== authGeneration.current) return; chooseSession(s); setModal(null); refreshSafely() }} />}
  </div>
}
