import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import assert from 'node:assert/strict'
import test from 'node:test'
import { accessControls, accessRequest, newAccess } from '../src/access.ts'

const roots = [{ id: 'main', name: 'Main', path: '/project/main' }, { id: 'extra', name: 'Extra', path: '/project/extra' }]
const option = id => ({ id, label: id, description: 'Test scope', unrestricted: id === 'full_access' })
const catalog = (id, modes) => ({ id, label: id, available: true, billing: '', access_modes: modes.map(option), supports_extra_roots: true, supports_network_control: true })
const codex = catalog('codex', ['inherit', 'read_only', 'workspace_write', 'full_access'])
const claude = catalog('claude', ['inherit', 'manual', 'plan', 'accept_edits', 'full_access'])
const hermes = catalog('hermes', ['inherit', 'full_access'])

test('inherit keeps CLI defaults: no hidden approval, network or extra grants', () => {
  const stale = { ...newAccess(), approval: 'never', network_access: true, additional_root_ids: ['extra'], acknowledge_full_access: true }
  assert.deepEqual(accessRequest(codex, roots, 'main', stale), { mode: 'inherit', additional_root_ids: [], acknowledge_full_access: false })
  assert.deepEqual(accessRequest(claude, roots, 'main', stale), { mode: 'inherit', additional_root_ids: [], acknowledge_full_access: false })
})

test('Codex workspace mode has explicit safe network default and scoped extra roots', () => {
  assert.deepEqual(accessRequest(codex, roots, 'main', newAccess('workspace_write')), {
    mode: 'workspace_write', approval: 'on_request', network_access: false, additional_root_ids: [], acknowledge_full_access: false,
  })
  const draft = { ...newAccess('workspace_write'), additional_root_ids: ['extra', 'extra'], network_access: true }
  assert.deepEqual(accessRequest(codex, roots, 'main', draft).additional_root_ids, ['extra'])
  assert.equal(accessRequest(codex, roots, 'main', draft).network_access, true)
  for (const forbidden of ['main', 'not-in-project']) {
    assert.throws(() => accessRequest(codex, roots, 'main', { ...draft, additional_root_ids: [forbidden] }), /Набор папок изменился/)
  }
})

test('full access requires acknowledgement and never carries workspace-only grants', () => {
  assert.throws(() => accessRequest(codex, roots, 'main', newAccess('full_access')), /Подтвердите/)
  const draft = { ...newAccess('full_access'), approval: 'never', network_access: true, additional_root_ids: ['extra'], acknowledge_full_access: true }
  assert.deepEqual(accessRequest(codex, roots, 'main', draft), {
    mode: 'full_access', approval: 'never', additional_root_ids: [], acknowledge_full_access: true,
  })
})

test('Claude extra roots only apply to manual, plan and accept edits modes', () => {
  for (const mode of ['manual', 'plan', 'accept_edits']) {
    const request = accessRequest(claude, roots, 'main', { ...newAccess(mode), additional_root_ids: ['extra'], network_access: true, approval: 'never' })
    assert.deepEqual(request, { mode, additional_root_ids: ['extra'], acknowledge_full_access: false })
    assert.equal(accessControls(claude, mode).network, false)
    assert.equal(accessControls(claude, mode).approval, false)
  }
  assert.equal(accessControls(claude, 'full_access').extras, false)
})

test('Hermes never shows or sends unsupported additional controls', () => {
  const draft = { ...newAccess('full_access'), approval: 'never', network_access: true, additional_root_ids: ['extra'], acknowledge_full_access: true }
  assert.deepEqual(accessRequest(hermes, roots, 'main', draft), { mode: 'full_access', additional_root_ids: [], acknowledge_full_access: true })
  assert.equal(accessControls(hermes, 'full_access').extras, false)
  assert.equal(accessControls(hermes, 'full_access').approval, false)
  assert.equal(accessControls(hermes, 'full_access').network, false)
})

test('catalog capability removal suppresses controls and stale grant fields', () => {
  const restricted = { ...codex, supports_extra_roots: false, supports_network_control: false }
  const request = accessRequest(restricted, roots, 'main', { ...newAccess('workspace_write'), additional_root_ids: ['extra'], network_access: true })
  assert.deepEqual(request, { mode: 'workspace_write', approval: 'on_request', additional_root_ids: [], acknowledge_full_access: false })
  assert.throws(() => accessRequest(codex, roots, 'main', newAccess('manual')), /больше недоступен/)
})

test('old daemon has no editable grants and receives no access override', () => {
  const legacy = { id: 'codex', label: 'Codex', available: true, billing: '' }
  assert.equal(accessControls(legacy, 'inherit').hasCatalog, false)
  assert.equal(accessRequest(legacy, roots, 'main', newAccess()), undefined)
})

test('fresh context reset revokes acknowledgement and all previous grants', () => {
  const first = newAccess('full_access')
  first.acknowledge_full_access = true
  first.additional_root_ids.push('extra')
  const second = newAccess()
  assert.deepEqual(second, { mode: 'inherit', approval: 'on_request', network_access: false, additional_root_ids: [], acknowledge_full_access: false })
  assert.notEqual(second.additional_root_ids, first.additional_root_ids)
})
