import test from 'node:test'
import assert from 'node:assert/strict'
import { mobileClient, forgetMobileConnection, cancelMobilePairing } from '../src/native-mobile.ts'
import { api, ApiError, initialToken, rememberVerifiedToken } from '../src/api.ts'

test('mobile API uses Rust IPC without renderer tokens or arbitrary URL forwarding', async () => {
  const previous = globalThis.window
  const calls = []
  globalThis.window = {
    __MPAC_NATIVE_MOBILE__: true,
    __TAURI__: { core: { invoke: async (...args) => { calls.push(args); return { status: 200, body: '{"ok":true}' } } } },
  }
  try {
    assert.equal(mobileClient(), true)
    assert.equal(initialToken(), 'native-mobile')
    rememberVerifiedToken('renderer-placeholder')
    assert.deepEqual(await api('renderer-placeholder', '/sessions/session-1/input', {data:'hello\r'}), {ok:true})
    assert.deepEqual(calls, [['mobile_request', {method:'POST',path:'/api/sessions/session-1/input',body:'{"data":"hello\\r"}'}]])
    assert.ok(!JSON.stringify(calls).includes('renderer-placeholder'))
  } finally { globalThis.window = previous }
})

test('revoked phone leaves mobile workspace and never falls back to a daemon token', async () => {
  const previous = globalThis.window
  const events = []
  globalThis.window = {__MPAC_NATIVE_MOBILE__:true, dispatchEvent:e=>events.push(e.type), __TAURI__:{core:{invoke:async()=>({status:401,body:'{"error":"Device access revoked"}'})}}}
  try {
    await assert.rejects(api('native-mobile','/state'), error=>error instanceof ApiError && error.status===401)
    assert.deepEqual(events,['mpac-mobile-access-lost'])
  } finally { globalThis.window = previous }
})

test('failed secure-store deletion preserves the mounted workspace; success disconnects', async () => {
  const previous=globalThis.window
  const events=[]
  globalThis.window={__MPAC_NATIVE_MOBILE__:true,dispatchEvent:e=>events.push(e.type),__TAURI__:{core:{invoke:async()=>{throw new Error('store locked')}}}}
  try {
    await assert.rejects(forgetMobileConnection(),/store locked/)
    assert.deepEqual(events,[])
    globalThis.window.__TAURI__.core.invoke=async()=>undefined
    await forgetMobileConnection()
    assert.deepEqual(events,['mpac-mobile-disconnected'])
  } finally {globalThis.window=previous}
})

test('aborted mobile request is not dispatched or automatically repeated', async () => {
  const previous=globalThis.window
  let calls=0
  globalThis.window={__MPAC_NATIVE_MOBILE__:true,__TAURI__:{core:{invoke:async()=>{calls++;throw new Error('network lost')}}}}
  try {
    const abort=new AbortController();abort.abort()
    await assert.rejects(api('native-mobile','/sessions/1/input',{data:'x'},abort.signal),e=>e.name==='AbortError')
    assert.equal(calls,0)
    await assert.rejects(api('native-mobile','/sessions/1/input',{data:'x'}),/network lost/)
    assert.equal(calls,1)
  } finally {globalThis.window=previous}
})


test('stalled pairing cancellation releases the review UI and never retries the native command', async () => {
  const previous=globalThis.window
  let calls=0, finish
  globalThis.window={__MPAC_NATIVE_MOBILE__:true,__TAURI__:{core:{invoke:(command)=>{assert.equal(command,'mobile_cancel');calls++;return new Promise(resolve=>{finish=resolve})}}}}
  try {
    await assert.rejects(cancelMobilePairing(20),/timed out/)
    assert.equal(calls,1)
    finish()
    await Promise.resolve()
    assert.equal(calls,1)
  } finally {globalThis.window=previous}
})
