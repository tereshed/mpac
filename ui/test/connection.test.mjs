import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import { afterEach, test } from 'node:test'
import assert from 'node:assert/strict'
import { connectToDaemon } from '../src/api.ts'

const originalFetch = globalThis.fetch
const state = { projects: [], sessions: [], agents: [], prototype: true }
afterEach(() => { globalThis.fetch = originalFetch })

test('valid token is returned only after authenticated state succeeds', async () => {
  let requested = false
  globalThis.fetch = async (path, options) => {
    assert.equal(path, '/api/state')
    assert.equal(options.headers.Authorization, 'Bearer valid-test-token')
    requested = true
    return Response.json(state)
  }
  const result = await connectToDaemon(' valid-test-token ')
  assert.equal(requested, true)
  assert.equal(result.token, 'valid-test-token')
  assert.deepEqual(result.state, state)
})

test('rejected token produces an actionable error and the attempt can be retried', async () => {
  globalThis.fetch = async () => Response.json({ error: 'unauthorized' }, { status: 401 })
  await assert.rejects(connectToDaemon('expired-test-token'), /Токен не подошёл или устарел/)
  globalThis.fetch = async () => Response.json(state)
  assert.equal((await connectToDaemon('replacement-token')).token, 'replacement-token')
})

test('manual full launch link is parsed without fetching its destination', async () => {
  globalThis.fetch = async (path, options) => {
    assert.equal(path, '/api/state')
    assert.equal(options.headers.Authorization, 'Bearer link-token')
    return Response.json(state)
  }
  assert.equal((await connectToDaemon('https://unused.example/#token=link-token')).token, 'link-token')
  await assert.rejects(connectToDaemon('http://127.0.0.1:4317/'), /Вставьте токен/)
})

test('unavailable daemon and forbidden origin have distinct feedback', async () => {
  globalThis.fetch = async () => { throw new TypeError('Failed to fetch') }
  await assert.rejects(connectToDaemon('test-token'), /Не удалось связаться с локальным daemon/)
  globalThis.fetch = async () => Response.json({ error: 'forbidden' }, { status: 403 })
  await assert.rejects(connectToDaemon('test-token'), /отклонено по адресу или Origin/)
})

test('a hung connection aborts and reports a timeout', async () => {
  let aborted = false
  globalThis.fetch = async (_path, options) => new Promise((_resolve, reject) => {
    options.signal.addEventListener('abort', () => { aborted = true; reject(options.signal.reason) }, { once: true })
  })
  await assert.rejects(connectToDaemon('test-token', undefined, 10), /не ответил за 10 секунд/)
  assert.equal(aborted, true)
})

test('an old attempt cannot succeed after its login form was replaced', async () => {
  const controller = new AbortController()
  globalThis.fetch = async () => {
    controller.abort()
    // Even a response racing with abort cannot become a successful login.
    return Response.json(state)
  }
  await assert.rejects(connectToDaemon('old-token', controller.signal), { name: 'AbortError' })
})

test('unexpected successful response is not accepted as a daemon login', async () => {
  globalThis.fetch = async () => Response.json({ status: 'unrelated service' })
  await assert.rejects(connectToDaemon('test-token'), /неожиданный ответ/)
})
