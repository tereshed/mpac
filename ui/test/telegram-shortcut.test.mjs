import test from 'node:test'
import assert from 'node:assert/strict'
import { telegramLinkSender } from '../src/telegram-shortcut.ts'

test('quick send performs one attempt and suppresses simultaneous clicks', async () => {
  let finish, calls = []
  const send = telegramLinkSender('renderer', body => { calls.push(body); return new Promise(resolve => { finish = resolve }) })
  const first = send()
  assert.equal(await send(), false)
  assert.equal(calls.length, 1)
  assert.deepEqual(Object.keys(calls[0]), ['request_id'])
  assert.match(calls[0].request_id, /^[a-f0-9]{64}$/)
  finish({sent: true}); assert.equal(await first, true)
})

test('uncertain delivery is not retried; a later explicit click gets a fresh delivery ID', async () => {
  const calls = []
  const send = telegramLinkSender('renderer', async body => { calls.push(body); throw new Error('offline') })
  await assert.rejects(send(), /offline/)
  assert.equal(calls.length, 1)
  await assert.rejects(send(), /offline/)
  assert.equal(calls.length, 2)
  assert.notEqual(calls[0].request_id, calls[1].request_id)
})

test('success requires an explicit positive delivery response', async () => {
  const send = telegramLinkSender('renderer', async () => ({sent: false}))
  await assert.rejects(send(), /not confirmed/)
})
