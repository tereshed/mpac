import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import assert from 'node:assert/strict'
import test from 'node:test'
import { needsAttention, nextSessionAfterRemoval, orderedSessions, readPreference, savePreference, sessionTitle } from '../src/session-management.ts'

const session = (id, status, project_id = 'project', started_at = 100) => ({ id, status, project_id, started_at })

test('session names validate UTF-8 bytes without splitting Cyrillic titles or accepting terminal controls', () => {
  assert.equal(sessionTitle('  Ревью авторизации  '), 'Ревью авторизации')
  assert.equal(sessionTitle('я'.repeat(60)), 'я'.repeat(60))
  assert.throws(() => sessionTitle('я'.repeat(61)), /120/)
  assert.throws(() => sessionTitle('  '), /Введите/)
  for (const value of ['one\ntwo', 'one\x1btwo', 'one\u009btwo', 'one\u202etwo']) assert.throws(() => sessionTitle(value), /управляющих/)
})

test('attention excludes naturally completed sessions and includes errors and interruptions', () => {
  assert.equal(needsAttention(session('1', 'exited')), false)
  assert.equal(needsAttention(session('1', 'running')), false)
  for (const status of ['needs_attention', 'failed', 'interrupted']) assert.equal(needsAttention(session('1', status)), true)
})
test('project sessions prioritize attention then live then history without mutating input', () => {
  const input = [session('old', 'stopped'), session('live', 'running'), session('reply', 'needs_attention'), session('new', 'exited', 'project', 110)]
  assert.deepEqual(orderedSessions(input).map(s => s.id), ['reply', 'live', 'new', 'old'])
  assert.deepEqual(input.map(s => s.id), ['old', 'live', 'reply', 'new'])
  assert.deepEqual(orderedSessions(input, true).map(s => s.id), ['reply'])
})
test('selection after deletion stays in the same project and never keeps archived session', () => {
  const items = [session('remove', 'stopped'), session('other-project', 'running', 'other'), session('history', 'exited'), session('live', 'running')]
  assert.equal(nextSessionAfterRemoval(items, 'project', 'remove').id, 'live')
  assert.equal(nextSessionAfterRemoval([items[0], items[1]], 'project', 'remove'), undefined)
})
test('sorting handles ISO timestamps and has stable tie-breaking', () => {
  const items = [session('b', 'stopped', 'project', '2026-11-03T01:00:00Z'), session('a', 'stopped', 'project', '2026-11-03T02:00:00Z')]
  assert.deepEqual(orderedSessions(items).map(s => s.id), ['a', 'b'])
})
test('view preferences tolerate unavailable browser storage', () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, get() { throw new Error('Storage blocked') } })
  try { assert.equal(readPreference('view', 'list'), 'list'); assert.doesNotThrow(() => savePreference('view', 'dots')) }
  finally { if (original) Object.defineProperty(globalThis, 'localStorage', original); else delete globalThis.localStorage }
})
