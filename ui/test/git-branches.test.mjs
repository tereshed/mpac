import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import assert from 'node:assert/strict'
import test from 'node:test'
import { branchIdentityChanged, branchSnapshotMatches, branchSwitchBlocked, branchSwitchRequest, currentBranchLabel } from '../src/git-branches.ts'

const main = {
  is_repo: true, current: 'main', head: '0123456789abcdef', detached: false, dirty: false,
  branches: [{ name: 'main', current: true }, { name: 'feature/review', current: false }], blocked_reason: null,
}

test('current labels distinguish normal, detached, unborn and non-repository folders', () => {
  assert.equal(currentBranchLabel(main), 'main')
  assert.equal(currentBranchLabel({ ...main, detached: true, current: null }), 'Отсоединённый HEAD · 01234567')
  assert.equal(currentBranchLabel({ ...main, head: 'unborn' }), 'main · без коммитов')
  assert.equal(currentBranchLabel({ ...main, head: null, current: null }), 'Ветка · без коммитов')
  assert.equal(currentBranchLabel({ ...main, is_repo: false }), 'Не Git-репозиторий')
})

test('switch request binds physical-root ID and the exact displayed HEAD and source branch', () => {
  assert.deepEqual(branchSwitchRequest(main, 'root-uuid', 'feature/review'), {
    root_id: 'root-uuid', branch: 'feature/review', expected_head: main.head, expected_branch: 'main',
  })
  // Branch names stay literal JSON values rather than shell fragments.
  const name = "feature/русский-'quoted'"
  const state = { ...main, branches: [...main.branches, { name, current: false }] }
  assert.equal(branchSwitchRequest(state, 'root-uuid', name).branch, name)
})

test('dirty worktrees cannot generate a switch even when an otherwise compatible target exists', () => {
  const state = { ...main, dirty: true }
  assert.match(branchSwitchBlocked(state), /незакоммиченные изменения/)
  assert.throws(() => branchSwitchRequest(state, 'root', 'feature/review'), /незакоммиченные изменения/)
})

test('active physical-repository and other server blocks remain authoritative', () => {
  const reason = 'Репозиторий используется работающей CLI-сессией другого проекта.'
  const state = { ...main, blocked_reason: reason }
  assert.equal(branchSwitchBlocked(state), reason)
  assert.throws(() => branchSwitchRequest(state, 'root', 'feature/review'), { message: reason })
})

test('unborn and non-Git roots never produce mutation requests', () => {
  assert.throws(() => branchSwitchRequest({ ...main, head: 'unborn' }, 'root', 'feature/review'), /первый коммит/)
  assert.throws(() => branchSwitchRequest({ ...main, head: null }, 'root', 'feature/review'), /первый коммит/)
  assert.throws(() => branchSwitchRequest({ ...main, is_repo: false }, 'root', 'feature/review'), /нет Git-репозитория/)
})

test('unknown and already-current targets cannot be dispatched', () => {
  assert.throws(() => branchSwitchRequest(main, 'root', 'not-a-listed-branch'), /больше недоступна/)
  assert.throws(() => branchSwitchRequest(main, 'root', main.current), /уже выбрана/)
  assert.match(branchSwitchBlocked({ ...main, branches: [main.branches[0]] }), /Других локальных веток/)
})

test('clean detached HEAD may switch to a listed local branch with expected_branch=null', () => {
  const state = { ...main, current: null, detached: true, branches: main.branches.map(branch => ({ ...branch, current: false })) }
  assert.equal(branchSwitchBlocked(state), '')
  assert.deepEqual(branchSwitchRequest(state, 'root', 'main'), {
    root_id: 'root', branch: 'main', expected_head: main.head, expected_branch: null,
  })
})

test('confirmation snapshot is invalidated by branch or commit changes, not by read object identity', () => {
  assert.equal(branchSnapshotMatches({ ...main }, main), true)
  assert.equal(branchSnapshotMatches({ ...main, head: 'new-head' }, main), false)
  assert.equal(branchSnapshotMatches({ ...main, current: 'other' }, main), false)
  assert.equal(branchSnapshotMatches(null, main), false)
  assert.equal(branchSnapshotMatches({ ...main, is_repo: false }, main), false)
})

test('background refresh invalidates files only for actual branch identity changes', () => {
  assert.equal(branchIdentityChanged(main, { ...main, dirty: true, blocked_reason: 'Работает CLI' }), false)
  assert.equal(branchIdentityChanged(main, { ...main, head: 'new-commit' }), true)
  assert.equal(branchIdentityChanged(main, { ...main, current: 'feature/review' }), true)
  assert.equal(branchIdentityChanged(main, { ...main, current: null, detached: true }), true)
  const plain = { ...main, is_repo: false, current: null, head: null, branches: [] }
  assert.equal(branchIdentityChanged(plain, { ...plain }), false)
  assert.equal(branchIdentityChanged(plain, main), true)
})
