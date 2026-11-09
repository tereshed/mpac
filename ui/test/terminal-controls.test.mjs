import test from 'node:test'
import assert from 'node:assert/strict'
import { terminalControlSequence } from '../src/terminal-controls.ts'

test('touch navigation respects normal and application cursor modes without submitting a selection', () => {
  for (const [key, suffix] of [['up','A'],['down','B'],['right','C'],['left','D']]) {
    assert.equal(terminalControlSequence(key), '\x1b[' + suffix)
    assert.equal(terminalControlSequence(key, true), '\x1bO' + suffix)
    assert.ok(!terminalControlSequence(key).includes('\r'))
  }
})
test('checkbox, field navigation, cancellation and confirmation are separate explicit keys', () => {
  for (const [action, sequence] of [['space',' '],['tab','\t'],['escape','\x1b'],['backspace','\x7f'],['submit','\r']]) {
    assert.equal(terminalControlSequence(action), sequence)
    assert.equal(terminalControlSequence(action,true), sequence)
  }
})
