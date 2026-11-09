import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage, query } from './api'
import Modal from './Modal'
import { branchIdentityChanged, branchSnapshotMatches, branchSwitchBlocked, branchSwitchRequest, currentBranchLabel } from './git-branches'
import type { GitBranches } from './git-branches'
import './branch-select.css'

interface Candidate { branch: string; expected: GitBranches }

export default function BranchSelect({ token, projectId, rootId, revision, onBeforeSwitch, onAfterSwitch, onExternalChange, onOpenCommit, onStateChange, onDialogChange }: {
  token: string; projectId: string; rootId: string; revision: number;
  onBeforeSwitch: () => void; onAfterSwitch: (switched: boolean) => void; onExternalChange: () => void;
  onOpenCommit?: () => void;
  onStateChange?: (state: GitBranches) => void;
  onDialogChange?: (open: boolean) => void;
}) {
  const [state, setState] = useState<GitBranches | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [candidate, setCandidate] = useState<Candidate | null>(null)
  useEffect(() => { onDialogChange?.(!!candidate); return () => onDialogChange?.(false) }, [candidate, onDialogChange])
  const [mutationError, setMutationError] = useState('')
  const [busy, setBusy] = useState(false)
  const [retry, setRetry] = useState(0)
  const [poll, setPoll] = useState(0)
  const mounted = useRef(true)
  const pending = useRef(false)
  const generation = useRef(0)
  const mutation = useRef<AbortController | null>(null)
  const listing = useRef<AbortController | null>(null)
  const confirmed = useRef<GitBranches | null>(null)
  const context = token + '\0' + projectId + '\0' + rootId
  const latestContext = useRef(context)
  if (latestContext.current !== context) { latestContext.current = context; generation.current += 1 }

  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false; mutation.current?.abort() }
  }, [])
  useEffect(() => {
    const timer = setInterval(() => { if (!pending.current) setPoll(value => value + 1) }, 7000)
    return () => clearInterval(timer)
  }, [])
  useEffect(() => {
    if (pending.current) return
    const abort = new AbortController()
    listing.current = abort
    const requested = ++generation.current
    if (!confirmed.current) setLoading(true)
    setError('')
    api<GitBranches>(token, `/projects/${projectId}/branches?${query({ root_id: rootId })}`, undefined, abort.signal)
      .then(result => {
        if (abort.signal.aborted || !mounted.current || requested !== generation.current) return
        if (confirmed.current && branchIdentityChanged(confirmed.current, result)) { onExternalChange(); setNotice('') }
        confirmed.current = result; setState(result); onStateChange?.(result)
      })
      .catch(failure => { if (!abort.signal.aborted && mounted.current && requested === generation.current) { setState(null); setError(errorMessage(failure)) } })
      .finally(() => { if (!abort.signal.aborted && mounted.current && requested === generation.current) setLoading(false) })
    return () => { abort.abort(); if (listing.current === abort) listing.current = null }
  }, [token, projectId, rootId, revision, retry, poll])

  const choose = (branch: string) => {
    if (!state || pending.current || loading || error || branch === state.current || !branch) return
    try {
      if (!state.branches.some(item => item.name === branch)) return
      setCandidate({ branch, expected: state }); setMutationError(''); setNotice('')
    } catch (failure) { setError(errorMessage(failure)) }
  }
  const switchBranch = async () => {
    if (!candidate || pending.current) return
    if (loading || error || !state || !branchSnapshotMatches(state, candidate.expected) || branchSwitchBlocked(state)) return
    let request
    try { branchSwitchRequest(state, rootId, candidate.branch); request = branchSwitchRequest(candidate.expected, rootId, candidate.branch) }
    catch (failure) { setMutationError(errorMessage(failure)); return }
    const abort = new AbortController()
    mutation.current = abort; pending.current = true
    listing.current?.abort()
    generation.current += 1 // An earlier branch-list read cannot overwrite the new result.
    setBusy(true); setMutationError(''); onBeforeSwitch()
    let switched = false
    try {
      const result = await api<GitBranches>(token, `/projects/${projectId}/branch`, request, abort.signal)
      if (!mounted.current || abort.signal.aborted || latestContext.current !== context) return
      switched = true; confirmed.current = result; setState(result); onStateChange?.(result); setCandidate(null); setNotice(t("Выбрана ветка «{p0}».", { p0: candidate.branch }))
    } catch (failure) {
      if (mounted.current && !abort.signal.aborted && latestContext.current === context) setMutationError(errorMessage(failure))
    } finally {
      pending.current = false
      if (mutation.current === abort) mutation.current = null
      if (mounted.current && latestContext.current === context) {
        setBusy(false); setLoading(false); onAfterSwitch(switched)
      }
    }
  }

  const blocked = state ? branchSwitchBlocked(state) : ''
  const currentPresent = state?.branches.some(branch => branch.name === state.current)
  const staleCandidate = !!candidate && !!state && !branchSnapshotMatches(state, candidate.expected)
  const missingCandidate = !!candidate && !!state && !state.branches.some(branch => branch.name === candidate.branch)
  const confirmDisabled = busy || loading || !state || !!error || !!blocked || staleCandidate || missingCandidate
  return <div className="branch-control">
    {state?.is_repo === false ? <span className="branch-non-repo">{t("⑂ Не Git-репозиторий")}</span> : <label className="branch-picker"><span>{t("⑂ Ветка")}</span><select aria-label={t("Ветка Git")} value={state?.current || ''} disabled={loading || busy || !state || !!error} onChange={event => choose(event.target.value)} aria-describedby={blocked || error ? 'git-branch-description' : undefined}>
      {(!state?.current || !currentPresent) && <option value={state?.current || ''}>{loading ? t("Загружаем…") : state ? currentBranchLabel(state) : t("Недоступна")}</option>}
      {state?.branches.map(branch => <option key={branch.name} value={branch.name}>{branch.name}{branch.current ? state.head === 'unborn' ? t(" · без коммитов") : t(" · текущая") : ''}</option>)}
    </select></label>}
    {blocked && state?.is_repo && <details className="branch-blocked-details" onKeyDown={event => { if (event.key === 'Escape') { event.currentTarget.open = false; event.stopPropagation() } }}><summary title={t(blocked)} aria-label={t('Ограничения переключения ветки')}>ⓘ</summary><p className="branch-blocked" id="git-branch-description">{t(blocked)}</p></details>}
    {error && <div className="branch-read-error" id="git-branch-description" role="status"><span>{t(error)}</span><button type="button" disabled={busy || loading} onClick={() => setRetry(value => value + 1)}>{t("Повторить")}</button></div>}
    {notice && <span className="branch-notice" role="status">{t(notice)}</span>}
    {candidate && <Modal title={t("Переключить ветку?")} subtitle={t("Git изменит файлы в выбранной рабочей папке.")} onClose={() => { if (!pending.current) { setCandidate(null); setMutationError('') } }} busy={busy}>
      <div className="branch-confirm-path"><span>{t("Текущая")}</span><code>{currentBranchLabel(candidate.expected)}</code><span>{t("Новая ветка")}</span><code>{candidate.branch}</code></div>
      <p className="branch-confirm-help">{t("Переключение изменит содержимое файлов рабочей папки. Операция недоступна, если есть незакоммиченные изменения или этот репозиторий используется работающей CLI-сессией.")}</p>
      <p className="branch-confirm-help">{t("Изменения не удаляются принудительно. Создание, загрузка и публикация веток здесь не выполняются.")}</p>
      {mutationError && <div className="inline-error" role="alert">{t(mutationError)}</div>}
      {!busy && staleCandidate && <div className="notice">{t("Текущая ветка или коммит изменились. Закройте окно и выберите ветку заново.")}</div>}
      {!busy && missingCandidate && <div className="notice">{t("Выбранной ветки больше нет в локальном списке. Закройте окно и выберите другую ветку.")}</div>}
      {!busy && blocked && !mutationError && <div className="notice">{t(blocked)}</div>}
      <div className="modal-actions"><button type="button" className="button secondary" disabled={busy} onClick={() => { if (!pending.current) setCandidate(null) }}>{t("Отмена")}</button>{state?.dirty && onOpenCommit && <button type="button" className="button secondary" disabled={busy} onClick={() => { setCandidate(null); onOpenCommit() }}>{t("Открыть коммит")}</button>}<button type="button" className="button primary" disabled={confirmDisabled} onClick={() => void switchBranch()}>{busy ? t("Переключаем…") : t("Переключить ветку")}</button></div>
    </Modal>}
  </div>
}
