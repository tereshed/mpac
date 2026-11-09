import assert from 'node:assert/strict'
import test from 'node:test'
import { activeProjectSessions, missingProtectedRoots, projectPaths, protectedProjectRoots, reconcileProjectSelection } from '../src/project-management.ts'

const main = { id: 'main', name: 'Main', path: '/project/main' }
const extra = { id: 'extra', name: 'Extra', path: '/project/extra ' }
const spare = { id: 'spare', name: 'Spare', path: '/project/spare' }
const project = { id: 'project', name: 'Project', roots: [main, extra, spare] }
const session = (values = {}) => ({ id: 'session', project_id: project.id, root_id: main.id, status: 'running', additional_roots: [extra], ...values })

test('editor preserves exact selected paths and only trims manual lines', () => {
  assert.deepEqual(projectPaths([main.path, extra.path], `${main.path}\n /new/manual \n\n${main.path}`), [main.path, extra.path, '/new/manual'])
  assert.deepEqual(projectPaths(['/same', '/same '], ''), ['/same', '/same '])
})

test('only active sessions in this project block deletion', () => {
  const sessions = [session(), session({ id: 'attention', status: 'needs_attention' }), session({ id: 'done', status: 'exited' }), session({ id: 'stopped', status: 'stopped' }), session({ id: 'other-project', project_id: 'other' })]
  assert.deepEqual(activeProjectSessions(project.id, sessions).map(item => item.id), ['session', 'attention'])
})

test('active main and explicit additional root IDs lock exactly those folders', () => {
  assert.deepEqual(protectedProjectRoots(project, [session()]).map(root => root.id), ['main', 'extra'])
  assert.deepEqual(protectedProjectRoots(project, [session({ additional_roots: [] })]).map(root => root.id), ['main'])
  assert.deepEqual(protectedProjectRoots(project, [session({ status: 'exited' })]), [])
  assert.deepEqual(protectedProjectRoots(project, [session({ project_id: 'other' })]), [])
})

test('rename and adding roots stay allowed while active folder removal is rejected', () => {
  assert.deepEqual(missingProtectedRoots(project, [main.path, extra.path, '/new/root'], [session()]), [])
  assert.deepEqual(missingProtectedRoots(project, [main.path], [session()]), [extra])
  assert.deepEqual(missingProtectedRoots(project, [extra.path, spare.path], [session()]), [main])
  // Trimming a filesystem-selected identity must not make a protected root match.
  assert.deepEqual(missingProtectedRoots(project, [main.path, extra.path.trim()], [session()]), [extra])
})

test('new active references after editing began are detected against the current draft', () => {
  const draft = [main.path]
  assert.deepEqual(missingProtectedRoots(project, draft, []), [])
  assert.deepEqual(missingProtectedRoots(project, draft, [session()]), [extra])
})

test('unchanged root and session IDs remain selected after metadata edits', () => {
  const renamed = { ...project, name: 'Renamed' }
  assert.deepEqual(reconcileProjectSelection(renamed, main.id, 'session', [session()]), { rootId: main.id, sessionId: 'session' })
})

test('removed inactive root selects a remaining folder without retaining a stale session selection', () => {
  const smaller = { ...project, roots: [main] }
  assert.deepEqual(reconcileProjectSelection(smaller, extra.id, 'history', [session({ id: 'history', root_id: extra.id, status: 'exited' })]), { rootId: main.id, sessionId: '' })
})

test('empty roots and sessions belonging to another project do not create phantom selection', () => {
  assert.deepEqual(reconcileProjectSelection({ ...project, roots: [] }, 'missing', 'unknown', []), { rootId: '', sessionId: '' })
  assert.deepEqual(reconcileProjectSelection(project, main.id, 'session', [session({ project_id: 'other' })]), { rootId: main.id, sessionId: '' })
})
