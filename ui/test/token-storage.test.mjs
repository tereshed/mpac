import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import { afterEach, test } from 'node:test'
import assert from 'node:assert/strict'
import { ApiError, authenticatedState, connectToDaemon, forgetRememberedToken, initialToken, reconnectToDaemon, rememberedToken, rememberVerifiedToken } from '../src/api.ts'

const keys = ['location', 'history', 'localStorage', 'sessionStorage', 'fetch']
const original = new Map(keys.map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]))
const state = { projects: [], sessions: [], agents: [], prototype: true }
afterEach(() => {
  for (const key of keys) {
    const descriptor = original.get(key)
    if (descriptor) Object.defineProperty(globalThis, key, descriptor)
    else delete globalThis[key]
  }
})

function storage(initial = {}) {
  const values = new Map(Object.entries(initial))
  return {
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: key => values.delete(key),
  }
}

function environment({ local = storage(), session = storage(), hash = '' } = {}) {
  const location = { hash, pathname: '/', search: '' }
  const history = { replaceState: (_state, _unused, url) => { location.hash = new URL(url, 'http://127.0.0.1:4317').hash } }
  for (const [key, value] of Object.entries({ localStorage: local, sessionStorage: session, location, history })) {
    Object.defineProperty(globalThis, key, { configurable: true, writable: true, value })
  }
  return { local, session, location }
}

test('durable token wins over a stale legacy token and survives a new tab', () => {
  const { local } = environment({ local: storage({ 'mpac-token': 'durable-good' }), session: storage({ 'mpac-token': 'stale-tab' }) })
  assert.equal(initialToken(), 'durable-good')
  Object.defineProperty(globalThis, 'sessionStorage', { configurable: true, value: storage() })
  assert.equal(initialToken(), 'durable-good')
  assert.equal(local.getItem('mpac-token'), 'durable-good')
})

test('an unverified or rejected fragment never overwrites remembered good credentials', async () => {
  const { local, location } = environment({ local: storage({ 'mpac-token': 'remembered-good' }), hash: '#token=bad-candidate' })
  assert.equal(initialToken(), 'bad-candidate')
  assert.equal(location.hash, '#token=bad-candidate')
  assert.equal(local.getItem('mpac-token'), 'remembered-good')
  globalThis.fetch = async () => Response.json({ error: 'unauthorized' }, { status: 401 })
  await assert.rejects(connectToDaemon('bad-candidate'), /Токен не подошёл/)
  assert.equal(local.getItem('mpac-token'), 'remembered-good')
  assert.equal(rememberedToken(), 'remembered-good')
})

test('a new fragment is committed only after authenticated state succeeds', async () => {
  const { local, session } = environment({ local: storage({ 'mpac-token': 'old-good' }), session: storage({ 'mpac-token': 'legacy' }), hash: '#token=new-verified' })
  const candidate = initialToken()
  let respond
  globalThis.fetch = async () => new Promise(resolve => { respond = resolve })
  const pending = authenticatedState(candidate)
  assert.equal(local.getItem('mpac-token'), 'old-good')
  respond(Response.json(state))
  assert.deepEqual(await pending, state)
  rememberVerifiedToken(candidate)
  assert.equal(local.getItem('mpac-token'), 'new-verified')
  assert.equal(session.getItem('mpac-token'), null)
})

test('legacy session credentials migrate only after successful verification', async () => {
  const { local, session } = environment({ session: storage({ 'mpac-token': 'legacy-verified' }) })
  const candidate = initialToken()
  assert.equal(candidate, 'legacy-verified')
  assert.equal(local.getItem('mpac-token'), null)
  globalThis.fetch = async () => Response.json(state)
  const verified = await connectToDaemon(candidate)
  rememberVerifiedToken(verified.token)
  assert.equal(local.getItem('mpac-token'), 'legacy-verified')
  assert.equal(session.getItem('mpac-token'), null)
})

test('unexpected state and aborted authentication leave the durable cache untouched', async () => {
  const { local } = environment({ local: storage({ 'mpac-token': 'good' }) })
  globalThis.fetch = async () => Response.json({ unrelated: true })
  await assert.rejects(authenticatedState('candidate'), /неожиданный ответ/)
  const abort = new AbortController()
  globalThis.fetch = async () => { abort.abort(); return Response.json(state) }
  await assert.rejects(authenticatedState('candidate', abort.signal), { name: 'AbortError' })
  assert.equal(local.getItem('mpac-token'), 'good')
})

test('logout clears both durable and legacy remembered credentials', () => {
  const { local, session } = environment({ local: storage({ 'mpac-token': 'good' }), session: storage({ 'mpac-token': 'legacy' }) })
  forgetRememberedToken()
  assert.equal(local.getItem('mpac-token'), null)
  assert.equal(session.getItem('mpac-token'), null)
  assert.equal(initialToken(), '')
})

test('blocked local writes use legacy storage without an old durable token masking the fallback', () => {
  const local = storage({ 'mpac-token': 'old' })
  local.setItem = () => { throw new DOMException('Full', 'QuotaExceededError') }
  const { session } = environment({ local })
  rememberVerifiedToken('new-verified')
  assert.equal(local.getItem('mpac-token'), null)
  assert.equal(session.getItem('mpac-token'), 'new-verified')
  assert.equal(rememberedToken(), 'new-verified')
})

test('throwing storage getters do not prevent fragment auth, in-memory commits or logout', () => {
  const { location } = environment({ hash: '#token=memory-only' })
  for (const key of ['localStorage', 'sessionStorage']) {
    Object.defineProperty(globalThis, key, { configurable: true, get() { throw new DOMException('Blocked', 'SecurityError') } })
  }
  assert.equal(initialToken(), 'memory-only')
  assert.equal(location.hash, '#token=memory-only')
  assert.doesNotThrow(() => rememberVerifiedToken('memory-only'))
  assert.doesNotThrow(forgetRememberedToken)
  assert.equal(rememberedToken(), '')
})

test('throwing storage methods and missing storage are handled safely', () => {
  const unavailable = { getItem() { throw Error('blocked') }, setItem() { throw Error('blocked') }, removeItem() { throw Error('blocked') } }
  environment({ local: unavailable, session: unavailable })
  assert.equal(initialToken(), '')
  assert.doesNotThrow(() => rememberVerifiedToken('verified'))
  assert.doesNotThrow(forgetRememberedToken)
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: undefined })
  Object.defineProperty(globalThis, 'sessionStorage', { configurable: true, value: undefined })
  assert.equal(initialToken(), '')
  assert.doesNotThrow(() => rememberVerifiedToken('verified'))
})

test('empty fragments and empty commits cannot erase a remembered good token', () => {
  const { local } = environment({ local: storage({ 'mpac-token': 'good' }), hash: '#token=%20%20' })
  assert.equal(initialToken(), 'good')
  rememberVerifiedToken('')
  assert.equal(local.getItem('mpac-token'), 'good')
})

test('an obsolete launch link automatically reconnects using remembered verified credentials', async () => {
  const { local } = environment({ local: storage({ 'mpac-token': 'current-good' }), hash: '#token=obsolete-link' })
  const calls = []
  globalThis.fetch = async (_path, options) => {
    calls.push(options.headers.Authorization)
    return calls.length === 1 ? Response.json({}, { status: 401 }) : Response.json(state)
  }
  const verified = await reconnectToDaemon(initialToken())
  assert.deepEqual(calls, ['Bearer obsolete-link', 'Bearer current-good'])
  assert.deepEqual(verified, { token: 'current-good', state })
  assert.equal(local.getItem('mpac-token'), 'current-good')
})

test('automatic auth recovery is bounded when both candidates are rejected', async () => {
  environment({ local: storage({ 'mpac-token': 'also-expired' }) })
  let calls = 0
  globalThis.fetch = async () => { calls++; return Response.json({}, { status: 401 }) }
  await assert.rejects(reconnectToDaemon('expired-link'), error => error instanceof ApiError && error.status === 401)
  assert.equal(calls, 2)
  calls = 0
  await assert.rejects(reconnectToDaemon('also-expired'))
  assert.equal(calls, 1)
})

test('forbidden origins and transient deployment outages never trigger credential fallback', async () => {
  environment({ local: storage({ 'mpac-token': 'good' }) })
  for (const status of [403, 503]) {
    let calls = 0
    globalThis.fetch = async () => { calls++; return Response.json({}, { status }) }
    await assert.rejects(reconnectToDaemon('candidate'), error => error.status === status)
    assert.equal(calls, 1)
  }
  let calls = 0
  globalThis.fetch = async () => { calls++; throw new TypeError('offline') }
  await assert.rejects(reconnectToDaemon('candidate'), TypeError)
  assert.equal(calls, 1)
})

test('logout or replacement abort prevents an automatic recovery from completing', async () => {
  const { local } = environment({ local: storage({ 'mpac-token': 'good' }) })
  const controller = new AbortController()
  let calls = 0
  globalThis.fetch = async () => {
    calls++
    if (calls === 1) return Response.json({}, { status: 401 })
    controller.abort()
    return Response.json(state)
  }
  await assert.rejects(reconnectToDaemon('old', controller.signal), { name: 'AbortError' })
  assert.equal(local.getItem('mpac-token'), 'good')
})

test('verified manual login survives reload when both browser stores are unavailable', async () => {
  const unavailable = { getItem() { throw Error('blocked') }, setItem() { throw Error('blocked') }, removeItem() { throw Error('blocked') } }
  const { location } = environment({ local: unavailable, session: unavailable })
  globalThis.fetch = async () => Response.json(state)
  assert.equal(initialToken(), '')
  const verified = await connectToDaemon('valid-manual-token')
  rememberVerifiedToken(verified.token)
  assert.equal(location.hash, '#token=valid-manual-token')
  // A new App mount reads the URL with no surviving storage state.
  assert.equal(initialToken(), 'valid-manual-token')
  assert.deepEqual(await reconnectToDaemon(initialToken()), verified)
  forgetRememberedToken()
  assert.equal(location.hash, '')
  assert.equal(initialToken(), '')
})

test('URL remains a reload recovery path when previously writable storage is cleared', () => {
  const { location, local } = environment()
  rememberVerifiedToken('verified-current')
  assert.equal(location.hash, '#token=verified-current')
  local.removeItem('mpac-token')
  assert.equal(initialToken(), 'verified-current')
})

test('unchanged verified connection does not repeatedly mutate browser history', () => {
  const { location } = environment()
  let calls = 0
  const replace = globalThis.history.replaceState
  globalThis.history.replaceState = (...args) => { calls++; replace(...args) }
  rememberVerifiedToken('verified')
  rememberVerifiedToken('verified')
  assert.equal(calls, 1)
  assert.equal(location.hash, '#token=verified')
  forgetRememberedToken()
  assert.equal(calls, 2)
  assert.equal(location.hash, '')
})

test('recovering an obsolete link replaces only its token after verification', async () => {
  const { location } = environment({ local: storage({ 'mpac-token': 'good' }), hash: '#token=old&view=files' })
  globalThis.fetch = async (_path, options) => options.headers.Authorization === 'Bearer good' ? Response.json(state) : Response.json({}, { status: 401 })
  const verified = await reconnectToDaemon(initialToken())
  assert.equal(location.hash, '#token=old&view=files')
  rememberVerifiedToken(verified.token)
  assert.equal(location.hash, '#token=good&view=files')
  forgetRememberedToken()
  assert.equal(location.hash, '#view=files')
})
