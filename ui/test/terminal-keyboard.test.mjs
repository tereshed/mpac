import assert from 'node:assert/strict'
import test from 'node:test'
import { directNewlineSequence, isTerminalNewlineKey, SHIFT_ENTER_SEQUENCE } from '../src/terminal-keyboard.ts'

const key = (changes = {}) => ({ type: 'keydown', key: 'Enter', shiftKey: true, ctrlKey: false, altKey: false, metaKey: false, isComposing: false, ...changes })

test('only plain Shift+Enter is intercepted; submit, other modifiers and IME keep native handling', () => {
  assert.equal(isTerminalNewlineKey(key()), true)
  for (const change of [{ shiftKey: false }, { key: 'a' }, { ctrlKey: true }, { altKey: true }, { metaKey: true }, { isComposing: true }, { keyCode: 229 }]) assert.equal(isTerminalNewlineKey(key(change)), false)
})
test('the whole key gesture is recognized so keypress/keyup cannot leak a raw Enter', () => {
  for (const type of ['keydown', 'keypress', 'keyup']) assert.equal(isTerminalNewlineKey(key({ type })), true)
})
test('native modified Enter remains distinguishable from submit with no bare CR or LF', () => {
  for (const agent of ['claude', 'zai', 'codex', 'hermes']) {
    const sequence = directNewlineSequence(agent)
    assert.equal(sequence, SHIFT_ENTER_SEQUENCE)
    assert.equal(Buffer.from(sequence).toString('hex'), '1b5b31333b3275')
    assert.equal(/[\r\n]/.test(sequence), false)
  }
  assert.equal(directNewlineSequence('demo'), null)
  assert.equal(directNewlineSequence('unknown'), null)
})
