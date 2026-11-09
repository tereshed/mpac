import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import assert from 'node:assert/strict'
import test from 'node:test'
import { accessPresetForStorage, cleanPresetAccess, defaultsSaveRequest, initialLaunchSelection, resolveLaunchPreset } from '../src/launch-defaults.ts'

const roots = [{ id: 'main', name: 'Main', path: '/project/main' }, { id: 'extra', name: 'Extra', path: '/project/extra' }, { id: 'third', name: 'Third', path: '/project/third' }]
const project = { id: 'project', name: 'Project', roots }
const modes = ['inherit', 'read_only', 'workspace_write', 'full_access']
const codex = { id: 'codex', available: true, access_modes: modes.map(id => ({ id, label: id, description: '', unrestricted: id === 'full_access' })), supports_extra_roots: true, supports_network_control: true }
const claude = { ...codex, id: 'claude', access_modes: ['inherit', 'manual', 'plan', 'accept_edits', 'full_access'].map(id => ({ id, label: id, description: '', unrestricted: id === 'full_access' })) }
const demo = { ...codex, id: 'demo', access_modes: [{ id: 'inherit', label: '', description: '', unrestricted: false }], supports_extra_roots: false, supports_network_control: false }
const inherit = { mode: 'inherit', additional_root_ids: [], acknowledge_full_access: false }
const full = { mode: 'full_access', approval: 'never', additional_root_ids: [], acknowledge_full_access: true }
const workspace = { mode: 'workspace_write', approval: 'on_request', network_access: true, additional_root_ids: ['main', 'extra', 'extra', 'removed'], acknowledge_full_access: true }
const catalog = () => ({ revision: 7, preferred_agent: 'codex', agents: { codex: workspace }, projects: { project: { agent_id: 'claude', root_id: 'extra', agents: { codex: { ...workspace, approval: 'never', additional_root_ids: ['third'] } } } } })

test('initial agent and root choices prefer valid project defaults over global and current selection', () => {
  const selected = initialLaunchSelection(project, [demo, codex, claude], 'main', catalog())
  assert.equal(selected.agentId, 'claude')
  assert.equal(selected.rootId, 'extra')
  assert.equal(selected.access.mode, 'inherit')
})

test('a review launch starts in its requested folder while preserving agent defaults and fresh access confirmation', () => {
  const defaults = catalog()
  defaults.projects.project.agent_id = 'codex'
  const chosen = initialLaunchSelection(project, [demo, codex, claude], 'main', defaults, 'third')
  assert.equal(chosen.agentId, 'codex')
  assert.equal(chosen.rootId, 'third')
  assert.equal(chosen.source, 'project')
  assert.equal(chosen.access.approval, 'never')
  assert.deepEqual(chosen.access.additional_root_ids, [])
  assert.equal(chosen.access.acknowledge_full_access, false)
  assert.equal(initialLaunchSelection(project, [demo, codex], 'main', defaults, 'removed').rootId, 'extra')
  assert.equal(defaults.projects.project.root_id, 'extra')
})

test('unavailable agents and removed folder preferences fall back to valid current choices', () => {
  const defaults = catalog()
  defaults.projects.project.root_id = 'removed'
  const selected = initialLaunchSelection(project, [demo, codex, { ...claude, available: false }], 'third', defaults)
  assert.equal(selected.agentId, 'codex')
  assert.equal(selected.rootId, 'third')
  assert.equal(initialLaunchSelection(project, [demo, codex], 'main', undefined).agentId, 'demo')
  assert.equal(initialLaunchSelection(project, [{ ...codex, available: false }], 'main', undefined).agentId, '')
})

test('project agent preset overrides global access and clears one-launch acknowledgement', () => {
  const resolved = resolveLaunchPreset(codex, project, 'main', catalog())
  assert.equal(resolved.source, 'project')
  assert.equal(resolved.access.approval, 'never')
  assert.deepEqual(resolved.access.additional_root_ids, ['third'])
  assert.equal(resolved.access.acknowledge_full_access, false)
})

test('global presets never carry root grants even if legacy data contains matching project IDs', () => {
  const defaults = catalog()
  delete defaults.projects.project.agents.codex
  const resolved = resolveLaunchPreset(codex, project, 'main', defaults)
  assert.equal(resolved.source, 'agent')
  assert.equal(resolved.access.network_access, true)
  assert.deepEqual(resolved.access.additional_root_ids, [])
})

test('changing main folder drops that ID and nonexistent or duplicate extra roots', () => {
  assert.deepEqual(cleanPresetAccess(codex, project, 'main', workspace).additional_root_ids, ['extra'])
  assert.deepEqual(cleanPresetAccess(codex, project, 'extra', workspace).additional_root_ids, ['main'])
  assert.deepEqual(cleanPresetAccess(codex, undefined, '', workspace).additional_root_ids, [])
})

test('full access can be saved as a mode but acknowledgement is never reusable', () => {
  const saved = accessPresetForStorage(codex, undefined, '', full)
  assert.equal(saved.mode, 'full_access')
  assert.equal(saved.acknowledge_full_access, false)
  const defaults = { revision: 1, preferred_agent: null, agents: { codex: full }, projects: {} }
  assert.equal(resolveLaunchPreset(codex, project, 'main', defaults).access.acknowledge_full_access, false)
  assert.equal(accessPresetForStorage(codex, project, 'main', { ...full, acknowledge_full_access: false }).acknowledge_full_access, false)
})

test('unknown stored modes fail closed to CLI defaults; invalid new saves are rejected', () => {
  assert.equal(cleanPresetAccess(codex, project, 'main', { ...inherit, mode: 'unknown-mode' }).mode, 'inherit')
  assert.throws(() => accessPresetForStorage(codex, project, 'main', { ...inherit, mode: 'manual' }), /поддерживаемый/)
  assert.throws(() => accessPresetForStorage(codex, project, 'removed', inherit), /существующую папку/)
})

test('save request targets one scope with frozen revision and explicit preferred-agent changes only', () => {
  const defaults = catalog()
  const global = defaultsSaveRequest(defaults, 'agent', codex, project, 'main', workspace, false)
  assert.equal(global.expected_revision, 7)
  assert.equal(global.agent_id, 'codex')
  assert.equal(global.scope, 'agent')
  assert.equal('project_id' in global, false)
  assert.equal('root_id' in global, false)
  assert.equal('make_default_agent' in global, false)
  assert.deepEqual(global.access.additional_root_ids, [])
  const specific = defaultsSaveRequest(defaults, 'project', codex, project, 'main', workspace, true)
  assert.equal(specific.project_id, project.id)
  assert.equal(specific.root_id, 'main')
  assert.equal(specific.make_default_agent, true)
  assert.equal(specific.access.acknowledge_full_access, false)
})

test('reset removes only the selected preset without changing default agent or folder', () => {
  assert.deepEqual(defaultsSaveRequest(catalog(), 'project', codex, project, 'third', null, true), {
    scope: 'project', agent_id: 'codex', project_id: project.id, access: null, expected_revision: 7,
  })
  assert.deepEqual(defaultsSaveRequest(catalog(), 'agent', codex, undefined, '', null, true), {
    scope: 'agent', agent_id: 'codex', access: null, expected_revision: 7,
  })
})

test('editing resolved controls never mutates the live defaults catalog', () => {
  const defaults = catalog()
  const original = JSON.stringify(defaults)
  const result = resolveLaunchPreset(codex, project, 'main', defaults)
  result.access.additional_root_ids.push('extra')
  result.access.approval = 'on_request'
  result.access.acknowledge_full_access = true
  assert.equal(JSON.stringify(defaults), original)
})
