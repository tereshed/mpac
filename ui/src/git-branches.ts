import { t } from './i18n.ts'
export interface GitBranches {
  is_repo: boolean
  current: string | null
  head: string | null
  detached: boolean
  dirty: boolean
  branches: { name: string; current: boolean }[]
  blocked_reason?: string | null
}

export function currentBranchLabel(state: GitBranches): string {
  if (!state.is_repo) return t("Не Git-репозиторий")
  if (!state.head || state.head === 'unborn') return t("{p0} · без коммитов", { p0: state.current || t("Ветка") })
  if (state.detached) return t("Отсоединённый HEAD · {p0}", { p0: state.head.slice(0, 8) })
  return state.current || state.head.slice(0, 8)
}

export function branchSwitchBlocked(state: GitBranches): string {
  if (!state.is_repo) return t("В этой папке нет Git-репозитория.")
  if (state.blocked_reason) return t(state.blocked_reason)
  if (state.dirty) return t("Есть незакоммиченные изменения. Переключение веток недоступно, пока они не сохранены средствами Git.")
  if (!state.head || state.head === 'unborn') return t("В репозитории ещё нет коммитов. Сначала создайте первый коммит средствами Git.")
  if (!state.branches.some(branch => branch.name !== state.current)) return t("Других локальных веток пока нет.")
  return ''
}

export function branchSwitchRequest(state: GitBranches, rootId: string, branch: string): {
  root_id: string; branch: string; expected_head: string; expected_branch: string | null;
} {
  const blocked = branchSwitchBlocked(state)
  if (blocked) throw new Error(blocked)
  if (branch === state.current) throw new Error(t("Эта ветка уже выбрана."))
  if (!state.branches.some(item => item.name === branch)) throw new Error(t("Ветка больше недоступна. Обновите список локальных веток."))
  if (!state.head || state.head === 'unborn') throw new Error(t("Не удалось определить текущий коммит."))
  return { root_id: rootId, branch, expected_head: state.head, expected_branch: state.current }
}

export function branchSnapshotMatches(current: GitBranches | null, expected: GitBranches): boolean {
  return !!current && current.is_repo && expected.is_repo && current.head === expected.head && current.current === expected.current
}

export function branchIdentityChanged(previous: GitBranches, next: GitBranches): boolean {
  return previous.is_repo !== next.is_repo || previous.current !== next.current || previous.head !== next.head || previous.detached !== next.detached
}
