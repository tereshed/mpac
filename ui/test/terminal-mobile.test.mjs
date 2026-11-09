import assert from 'node:assert/strict'
import test from 'node:test'
import { TerminalTouchScroll, terminalHistory, insertAndMaybeSubmit } from '../src/terminal-mobile.ts'

test('touch scrolling follows a finger, accumulates fractions and never turns a tap/horizontal gesture into scrolling', () => {
  const touch = new TerminalTouchScroll()
  touch.start(10, 100)
  assert.deepEqual(touch.move(11, 98, 18), { scrolling: false, lines: 0 })
  assert.deepEqual(touch.move(11, 80, 18), { scrolling: true, lines: 1 })
  assert.deepEqual(touch.move(11, 66, 18), { scrolling: true, lines: 0 })
  assert.deepEqual(touch.move(11, 60, 18), { scrolling: true, lines: 1 })
  assert.deepEqual(touch.move(11, 100, 18), { scrolling: true, lines: -2 })
  touch.end()
  assert.equal(touch.move(11, 0, 18).scrolling, false)
  touch.start(10, 100)
  assert.equal(touch.move(100, 95, 18).scrolling, false)
  assert.equal(touch.move(11, 80, 0).scrolling, false)
})
test('history snapshot is bounded, keeps Unicode and literal markup, and does not change while new output arrives', () => {
  const rows = ['old', 'Привет 👋', '<script>literal</script>', 'last', '  ']
  const buffer = { get length() { return rows.length }, getLine: i => ({ isWrapped: false, translateToString: () => rows[i] }) }
  const snapshot = terminalHistory(buffer)
  rows.push('new output')
  assert.equal(snapshot, 'old\nПривет 👋\n<script>literal</script>\nlast')
  assert.equal(terminalHistory(buffer, 2), '  \nnew output')
  assert.ok(terminalHistory(buffer, 5000, 12).length <= 12)
})
test('explicit send waits for acknowledged insertion; a lost insertion ack never submits or retries', async () => {
  const calls = []
  let accept
  const sending = insertAndMaybeSubmit('task', async text => { calls.push(text); await new Promise(resolve => { accept = resolve }) }, async () => { calls.push('\r') })
  assert.deepEqual(calls, ['task'])
  accept(); await sending
  assert.deepEqual(calls, ['task', '\r'])
  calls.length = 0
  await assert.rejects(insertAndMaybeSubmit('task', async text => { calls.push(text); throw new Error('lost ack') }, async () => { calls.push('\r') }), /lost ack/)
  assert.deepEqual(calls, ['task'])
})
