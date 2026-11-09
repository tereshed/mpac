import { t } from './i18n.ts'
import type { ActivityState } from './activity-types'
import { mobileClient, mobileInvoke } from './native-mobile.ts'

export interface Root { id: string; name: string; path: string }
export interface Project { id: string; name: string; roots: Root[] }
export type AccessMode = 'inherit' | 'read_only' | 'workspace_write' | 'plan' | 'manual' | 'accept_edits' | 'full_access'
export interface SessionAccess {
  mode: AccessMode
  approval?: 'on_request' | 'never'
  network_access?: boolean
  additional_root_ids: string[]
  acknowledge_full_access: boolean
}
export interface ProjectLaunchDefaults {
  agent_id: string | null;
  root_id: string | null;
  agents: Record<string, SessionAccess>;
}
export interface LaunchDefaults {
  revision: number;
  preferred_agent: string | null;
  agents: Record<string, SessionAccess>;
  projects: Record<string, ProjectLaunchDefaults>;
}
export interface AccessModeOption { id: AccessMode; label: string; description: string; unrestricted: boolean }
export interface Session {
  activity?: ActivityState;
  id: string; project_id: string; root_id: string; agent: string; title: string;
  cli_title?: string | null;
  title_source?: 'manual' | 'pending_first_answer' | 'harness';
  auto_title_eligible?: boolean;
  status: string; started_at: number | string; last_activity: number | string;
  exit_code?: number | null; error?: string | null; seq: number; mode: string;
  access?: SessionAccess;
  additional_roots?: Root[];
  permission_args?: string[];
  terminal_launch?: { shell: string; command: string | null } | null;
  environment?: { file: string; variable_count: number } | null;
}
export interface Agent {
  id: string; label: string; available: boolean; billing: string;
  access_modes?: AccessModeOption[];
  supports_extra_roots?: boolean;
  supports_network_control?: boolean;
}
export interface State {
  projects: Project[]; archived_projects?: Project[];
  sessions: Session[]; archived_sessions?: Session[]; agents: Agent[]; terminal_launches?: Agent[]; prototype: boolean;
  launch_defaults?: LaunchDefaults;
  capabilities?: { project_management?: boolean; [capability: string]: unknown };
}
export interface FileEntry { path: string; is_dir: boolean; size: number }
export interface Review { head: string; snapshot: string; diff: string; files: { path: string; status: string }[]; truncated: boolean }

const TOKEN_KEY = 'mpac-token'
type TokenStorage = 'localStorage' | 'sessionStorage'

function readToken(storage: TokenStorage): string {
  try { return globalThis[storage]?.getItem(TOKEN_KEY) || '' }
  catch { return '' }
}

function writeToken(storage: TokenStorage, token: string): boolean {
  try {
    const target = globalThis[storage]
    if (!target) return false
    target.setItem(TOKEN_KEY, token)
    return true
  } catch { return false }
}

function removeToken(storage: TokenStorage): void {
  try { globalThis[storage]?.removeItem(TOKEN_KEY) }
  catch { /* A browser may block storage; in-memory auth still works. */ }
}

/** Durable same-origin credentials take priority over legacy per-tab copies.
 * A legacy candidate is migrated only after the daemon verifies it. */
export function rememberedToken(): string {
  return readToken('localStorage') || readToken('sessionStorage')
}

function retainConnectionInUrl(token: string): void {
  try {
    const hash = new URLSearchParams(location.hash.slice(1))
    if (token) hash.set('token', token)
    else hash.delete('token')
    const fragment = hash.toString()
    const target = location.pathname + location.search + (fragment ? '#' + fragment : '')
    if (target === location.pathname + location.search + location.hash) return
    // replaceState does not fire hashchange or create a new auth generation.
    // A fragment stays on the client; it is never part of the HTTP request.
    history.replaceState(null, '', target)
  } catch { /* Restricted history must not prevent an in-memory connection. */ }
}

/** Called only after authenticated state validation and the caller's abort /
 * generation guards. An unverified URL fragment must never call this helper. */
export function rememberVerifiedToken(token: string): void {
  if (mobileClient()) return
  if (!token) return
  if (readToken('localStorage') === token || writeToken('localStorage', token)) {
    removeToken('sessionStorage')
  } else {
    // If quota blocks an update but permits removal, don't let an old durable
    // token mask this newer verified fallback on the next page load.
    removeToken('localStorage')
    writeToken('sessionStorage', token)
  }
  // Do not depend on storage surviving a reload (private/ephemeral profiles).
  // Keep the verified launch capability in this tab's URL as a recovery path.
  retainConnectionInUrl(token)
}

export function forgetRememberedToken(): void {
  if (mobileClient()) {
    return
  }
  removeToken('localStorage')
  removeToken('sessionStorage')
  retainConnectionInUrl('')
}

export function initialToken(): string {
  if (mobileClient()) return 'native-mobile'
  const supplied = typeof location === 'undefined' ? null : new URLSearchParams(location.hash.slice(1)).get('token')
  if (supplied?.trim()) {
    return supplied
  }
  return rememberedToken()
}

export class ApiError extends Error {
  status: number
  constructor(message: string, status: number) { super(message); this.status = status }
}

export async function authenticatedState(token: string, signal?: AbortSignal): Promise<State> {
  const state = await api<State>(token, '/state', undefined, signal)
  if (signal?.aborted) throw new DOMException('Aborted', 'AbortError')
  if (!state || !Array.isArray(state.projects) || !Array.isArray(state.sessions) || !Array.isArray(state.agents)) {
    throw new Error(t("Daemon вернул неожиданный ответ. Проверьте ссылку запуска."))
  }
  return state
}

/** Automatic reconnect may recover an obsolete launch link using the verified
 * browser credential. Manual login deliberately keeps its own error feedback.
 * No credentials are saved here: the caller must still check its auth epoch. */
export async function reconnectToDaemon(token: string, signal?: AbortSignal): Promise<{ token: string; state: State }> {
  try {
    return { token, state: await authenticatedState(token, signal) }
  } catch (error) {
    if (signal?.aborted || !(error instanceof ApiError) || error.status !== 401) throw error
    const remembered = rememberedToken()
    if (!remembered || remembered === token) throw error
    return { token: remembered, state: await authenticatedState(remembered, signal) }
  }
}

export function connectionErrorMessage(error: unknown): string {
  if (error instanceof ApiError && error.status === 401) return t("Токен не подошёл или устарел. Откройте актуальную ссылку запуска или вставьте новый токен.")
  if (error instanceof ApiError && error.status === 403) return t("Подключение отклонено по адресу или Origin. Откройте приложение по ссылке запуска с адресом 127.0.0.1.")
  if (error instanceof TypeError) return t("Не удалось связаться с локальным daemon. Убедитесь, что приложение запущено, и повторите подключение.")
  return errorMessage(error)
}

/** Verify a manual attempt before saving credentials or leaving the form. */
export async function connectToDaemon(value: string, signal?: AbortSignal, timeoutMs = 10000): Promise<{ token: string; state: State }> {
  let token = value.trim()
  if (/^https?:\/\//i.test(token)) {
    try { token = new URL(token).hash ? new URLSearchParams(new URL(token).hash.slice(1)).get('token') || '' : '' }
    catch { throw new Error(t("Не удалось прочитать ссылку подключения.")) }
  }
  if (!token) throw new Error(t("Вставьте токен или полную ссылку запуска с #token=…"))
  const controller = new AbortController()
  let timedOut = false
  const abort = () => controller.abort(signal?.reason)
  if (signal?.aborted) abort()
  signal?.addEventListener('abort', abort, { once: true })
  const timer = setTimeout(() => { timedOut = true; controller.abort() }, timeoutMs)
  try {
    // The pasted URL is only a token carrier: never send it to another host.
    const state = await authenticatedState(token, controller.signal)
    return { token, state }
  } catch (error) {
    if (signal?.aborted) throw error
    if (timedOut) throw new Error(t("Daemon не ответил за 10 секунд. Проверьте, что приложение запущено, и повторите подключение."))
    throw new Error(connectionErrorMessage(error))
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener('abort', abort)
  }
}

export async function api<T>(token: string, path: string, body?: unknown, signal?: AbortSignal): Promise<T> {
  if (mobileClient()) {
    if (signal?.aborted) throw new DOMException('Aborted', 'AbortError')
    const response = await mobileInvoke<{status: number; body: string}>('mobile_request', { method: body === undefined ? 'GET' : 'POST', path: '/api' + path, body: body === undefined ? null : JSON.stringify(body) })
    if (signal?.aborted) throw new DOMException('Aborted', 'AbortError')
    const value = response.body ? JSON.parse(response.body) : undefined
    if (response.status === 401) window.dispatchEvent(new Event('mpac-mobile-access-lost'))
    if (response.status < 200 || response.status >= 300) throw new ApiError(t(value?.error || `Error ${response.status}`), response.status)
    return value as T
  }
  const response = await fetch('/api' + path, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { Authorization: 'Bearer ' + token, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal,
    cache: 'no-store',
    credentials: 'omit',
  })
  if (!response.ok) {
    let message = t("Ошибка {p0}", { p0: response.status })
    try {
      const value = await response.json()
      message = value.error || value.message || message
    } catch { /* Do not render arbitrary HTML error bodies. */ }
    throw new ApiError(t(String(message)), response.status)
  }
  if (response.status === 204) return undefined as T
  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

export function query(values: Record<string, string>): string { return new URLSearchParams(values).toString() }
export function errorMessage(error: unknown): string { return error instanceof Error ? t(error.message) : t("Не удалось выполнить действие") }
export function isRunning(session: Session): boolean { return session.status === 'running' || session.status === 'needs_attention' }
export function statusLabel(status: string): string {
  return ({ running: t("Запущена"), needs_attention: t("Нужен ответ"), exited: t("Завершена"), failed: t("Ошибка"), stopped: t("Остановлена"), interrupted: t("Прервана") })[status] || status
}
export function secondsSince(value: string | number): number {
  const milliseconds = typeof value === 'number' ? (value < 1e12 ? value * 1000 : value) : Date.parse(value)
  return Number.isFinite(milliseconds) ? Math.max(0, Math.floor((Date.now() - milliseconds) / 1000)) : 0
}
export function age(value: string | number): string {
  const seconds = secondsSince(value)
  return seconds < 60 ? t("{p0} с", { p0: seconds }) : seconds < 3600 ? t("{p0} мин", { p0: Math.floor(seconds / 60) }) : t("{p0} ч", { p0: Math.floor(seconds / 3600) })
}
