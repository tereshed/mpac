import assert from 'node:assert/strict'
import test from 'node:test'
import { sourceLineWindow } from '../src/file-navigation.ts'

test('deep source links include the actual line while retaining bounded DOM size', () => {
  for (const line of [1, 1999, 2001, 19000, 25000, 50000]) {
    const window = sourceLineWindow(50000, line)
    assert.equal(window.found, true)
    assert.ok(window.start < line && window.end >= line)
    assert.ok(window.end - window.start <= 2000)
  }
})
test('missing and invalid source coordinates never pretend to highlight another line', () => {
  for (const line of [0, -1, 1.2, Number.NaN, Number.MAX_SAFE_INTEGER]) assert.deepEqual(sourceLineWindow(7, line), { start: 0, end: 7, found: false })
  assert.deepEqual(sourceLineWindow(0, 1), { start: 0, end: 0, found: false })
  assert.deepEqual(sourceLineWindow(7), { start: 0, end: 7, found: true })
})
