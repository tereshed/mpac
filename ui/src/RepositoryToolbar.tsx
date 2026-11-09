import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { Project, Review, Root } from './api'
import { api, query } from './api'
import { RootSelect } from './FilesPane'
import BranchSelect from './BranchSelect'
import CommitDialog from './CommitDialog'
import { currentBranchLabel } from './git-branches'
import type { GitBranches } from './git-branches'
import './repository-toolbar.css'

interface Props {
  token: string; project: Project; root: Root; onRoot: (id: string) => void; revision: number;
  branchesAvailable: boolean; commitsAvailable: boolean; onBeforeMutation: () => void;
  onAfterMutation: (changed: boolean) => void; onExternalChange: () => void; onCommitDone?: () => void; busy?: boolean; compact?: boolean; onDialogChange?: (open: boolean) => void;
}
export default function RepositoryToolbar({ token, project, root, onRoot, revision, branchesAvailable, commitsAvailable, onBeforeMutation, onAfterMutation, onExternalChange, onCommitDone, busy = false, compact = false, onDialogChange }: Props) {
  const [commitOpen, setCommitOpen] = useState(false)
  const [expanded, setExpanded] = useState(false)
  const [branchDialogOpen, setBranchDialogOpen] = useState(false)
  const [branchState, setBranchState] = useState<GitBranches | null>(null)
  const branchLabel = branchState ? currentBranchLabel(branchState) : 'Git'
  useEffect(() => { onDialogChange?.(commitOpen || branchDialogOpen); return () => onDialogChange?.(false) }, [commitOpen, branchDialogOpen, onDialogChange])
  const [count, setCount] = useState<number | null>(null)
  const [dirty, setDirty] = useState<boolean | null>(null)
  const [isRepo, setIsRepo] = useState<boolean | null>(null)
  const [refresh, setRefresh] = useState(0)
  const [mutating, setMutating] = useState(false)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => {
    const abort = new AbortController()
    api<Review>(token, `/projects/${project.id}/review?${query({ root_id: root.id })}`, undefined, abort.signal).then(review => { if (!abort.signal.aborted) setCount(review.files.length) }).catch(() => { if (!abort.signal.aborted) setCount(null) })
    return () => abort.abort()
  }, [token, project.id, root.id, revision, refresh])
  const begin = () => { setMutating(true); onBeforeMutation() }
  const finish = (changed: boolean) => { if (!mounted.current) return; setMutating(false); setRefresh(value => value + 1); onAfterMutation(changed); if (changed) onCommitDone?.() }
  return <>{compact && <button type="button" className="mobile-repository-toggle" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}><span>⑂ {branchLabel}</span><small>{root.name}{dirty ? t(" · изменения") : ''}</small><span aria-hidden="true">{expanded ? '⌃' : '⌄'}</span></button>}<div className={`repository-toolbar ${compact && !expanded && !branchDialogOpen ? 'mobile-collapsed' : ''}`}><RootSelect project={project} root={root} onChange={onRoot} disabled={busy || mutating} />{branchesAvailable && <BranchSelect token={token} projectId={project.id} rootId={root.id} revision={revision + refresh} onBeforeSwitch={begin} onAfterSwitch={finish} onExternalChange={onExternalChange} onOpenCommit={commitsAvailable ? () => setCommitOpen(true) : undefined} onDialogChange={setBranchDialogOpen} onStateChange={state => { setDirty(state.dirty); setIsRepo(state.is_repo); setBranchState(state) }} />}{commitsAvailable && <button type="button" className="button secondary compact" disabled={busy || mutating || isRepo === false || dirty === false || (!branchesAvailable && count === 0)} onClick={() => setCommitOpen(true)}>{t("Коммит…")}</button>}<details className="repository-details" onKeyDown={event => { if (event.key === 'Escape') { event.currentTarget.open = false; event.stopPropagation() } }}><summary aria-label={t('Сведения о репозитории')} title={t('Сведения о репозитории')}>⋯</summary><div className="repository-details-popover"><span className="repository-change-count">{isRepo === false ? t("Не Git-репозиторий") : dirty === false ? t("Рабочая папка чистая") : dirty === true ? t("Есть изменения") : count === null ? t("Проверяем Git…") : count ? t("Изменено файлов: {p0}", { p0: count }) : t("Рабочая папка чистая")}</span><span className="repository-path" title={root.path}>{root.path}</span><button type="button" className="icon-button" aria-label={t("Обновить состояние репозитория")} disabled={busy || mutating} onClick={() => { setRefresh(value => value + 1); onExternalChange() }}>↻</button></div></details></div>{commitOpen && <CommitDialog token={token} project={project} root={root} onClose={() => setCommitOpen(false)} onBeforeMutation={begin} onAfterMutation={finish} />}</>
}
