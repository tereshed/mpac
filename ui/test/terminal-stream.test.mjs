import assert from 'node:assert/strict'
import test from 'node:test'
import { TerminalStream, TerminalInputBatcher, TerminalRenderQueue, terminalInputChunks } from '../src/terminal-stream.ts'

const tick = () => new Promise(resolve => setTimeout(resolve, 0))
class Socket {
  readyState = 0; bufferedAmount = 0; sent = []; onopen; onclose; onerror; onmessage;
  open() { this.readyState = 1; this.onopen?.() }
  frame(value) { this.onmessage?.({ data: JSON.stringify(value) }) }
  send(value) { this.sent.push(JSON.parse(value)) }
  close() { this.readyState = 3 }
  disconnect() { this.readyState = 3; this.onclose?.() }
}
function fixture(overrides = {}) {
  const socket = new Socket(), output = [], errors = [];
  const stream = new TerminalStream({ url: 'ws://127.0.0.1:1234/terminal', token: 'private-ephemeral', sessionId: 'session-1', after: 7, native: true,
    socketFactory: () => socket, onOutput: async value => { output.push(value) }, onDisconnect: error => errors.push(error), onEnd: () => {}, ...overrides });
  stream.ready.catch(() => {});
  return { stream, socket, output, errors };
}
async function ready(f) { f.socket.open(); f.socket.frame({ type: 'ready' }); await f.stream.ready }

test('credentials travel only in the first frame; writes resolve only on their own acknowledgement', async () => {
  const f = fixture(); await ready(f);
  try {
    assert.deepEqual(f.socket.sent, [{ type: 'auth', token: 'private-ephemeral', session_id: 'session-1', after: 7 }]);
    let acknowledged = false;
    const input = f.stream.input('hello\x1b[13;2uworld').then(() => { acknowledged = true });
    await tick(); assert.equal(acknowledged, false);
    const sent = f.socket.sent.at(-1); assert.equal(sent.type, 'input');
    f.socket.frame({ type: 'ack', id: sent.id + 1 }); await tick(); assert.equal(acknowledged, false);
    f.socket.frame({ type: 'ack', id: sent.id }); await input; assert.equal(acknowledged, true);
    assert.equal(sent.data, 'hello\x1b[13;2uworld');
  } finally { f.stream.close() }
});

test('output is acknowledged only after the renderer consumes the exact UTF8 bytes', async () => {
  let release; const rendered = new Promise(resolve => { release = resolve });
  const f = fixture({ onOutput: () => rendered }); await ready(f);
  try {
    const data = Buffer.from('¡Привет!\x1b[32m');
    f.socket.frame({ type: 'output', data_base64: data.toString('base64'), seq: 7 + data.length, reset: false });
    await tick(); assert.equal(f.socket.sent.filter(x => x.type === 'ack').length, 0);
    release(); await tick(); assert.deepEqual(f.socket.sent.at(-1), { type: 'ack', seq: 7 + data.length });
  } finally { f.stream.close() }
});

test('a disconnect rejects uncertain input and never sends or replays it on another socket', async () => {
  const f = fixture(); await ready(f);
  const input = f.stream.input('dangerous command\r');
  const rejected = assert.rejects(input, /Неподтверждённый ввод/);
  await tick(); f.socket.disconnect(); await rejected;
  assert.equal(f.socket.sent.filter(x => x.type === 'input').length, 1);
  assert.equal(f.errors.length, 1);
  await assert.rejects(f.stream.input('\r'), /не готов/);
});

test('rejects output gaps, unsolicited pre-auth output and concurrent unacknowledged output', async () => {
  for (const kind of ['gap', 'before-ready', 'parallel']) {
    let release; const rendering = new Promise(resolve => { release = resolve });
    const f = fixture({ onOutput: () => rendering }); f.socket.open();
    if (kind !== 'before-ready') { f.socket.frame({ type: 'ready' }); await f.stream.ready }
    const frame = { type: 'output', data_base64: 'YQ==', seq: kind === 'gap' ? 11 : 8, reset: false };
    f.socket.frame(frame); if (kind === 'parallel') f.socket.frame(frame);
    await tick(); assert.equal(f.errors.length, 1, kind); assert.equal(f.socket.readyState, 3);
    release(); f.stream.close();
  }
});

test('normal terminal end does not request reconnect or replay', async () => {
  let ended = 0; const f = fixture({ onEnd: () => ended++ }); await ready(f);
  f.socket.frame({ type: 'end' }); await tick(); assert.equal(ended, 1); assert.equal(f.errors.length, 0); assert.equal(f.socket.readyState, 3);
});

test('short input bursts merge once with byte-identical control sequences and no implicit Enter', async () => {
  const sent = [], batcher = new TerminalInputBatcher(async data => { sent.push(data) });
  try { await Promise.all(['a', 'é', '\x1b[13;2u', '文'].map(value => batcher.enqueue(value))); assert.deepEqual(sent, ['aé\x1b[13;2u文']); assert.equal(sent[0].includes('\r'), false) }
  finally { batcher.dispose() }
});

test('input batch limits count UTF8 bytes, preserve paste boundaries, and never retry failed writes', async () => {
  const sent = []; let release;
  const gate = new Promise(resolve => { release = resolve });
  const batcher = new TerminalInputBatcher(async data => { sent.push(data); await gate; throw new Error('uncertain') });
  await assert.rejects(batcher.enqueue('文'.repeat(5500)), /16 KiB/);
  const first = batcher.enqueue('a'.repeat(16 * 1024)); const failed = assert.rejects(first, /uncertain/);
  await new Promise(resolve => setTimeout(resolve, 12));
  const pending = batcher.enqueue('\r'); const cancelled = assert.rejects(pending, /uncertain/);
  release(); await Promise.all([failed, cancelled]);
  assert.equal(sent.length, 1); assert.equal(sent[0].length, 16 * 1024); batcher.dispose();
});


test('escaped control bytes and Unicode chunks fit the wire limit without changing paste bytes', () => {
  for (const text of ['\x00'.repeat(16 * 1024), '\r\n'.repeat(8192), '\x1b[200~' + '¡文😀'.repeat(1000) + '\x1b[201~']) {
    const chunks = terminalInputChunks(text);
    assert.equal(chunks.join(''), text);
    for (const data of chunks) assert.ok(Buffer.byteLength(JSON.stringify({ type: 'input', id: 999999999999, data })) < 24 * 1024);
  }
});

test('browser first frame has only daemon auth fields, never native-only session metadata', async () => {
  const f = fixture({ native: false }); await ready(f);
  try { assert.deepEqual(f.socket.sent[0], { type: 'auth', token: 'private-ephemeral', after: 7 }) } finally { f.stream.close() }
});


test('reconnect cursor waits for a pending render after disconnect so bytes cannot replay twice', async () => {
  const queue = new TerminalRenderQueue(); let release, resumed = false;
  const slowRender = new Promise(resolve => { release = resolve });
  const write = queue.write({ seq: 123, data_base64: 'YQ==', reset: false }, () => slowRender);
  const reconnectCursor = queue.cursor().then(after => { resumed = true; return after });
  await tick(); assert.equal(resumed, false); assert.equal(queue.sequence, 0);
  release(); await write; assert.equal(await reconnectCursor, 123);
});

test('closing before readiness or during an acknowledged command rejects callers without later sends', async () => {
  for (const opened of [false, true]) {
    const f = fixture(); if (opened) await ready(f);
    const input = f.stream.input('once\r'); const rejected = assert.rejects(input, /закрыт/);
    await tick(); f.stream.close(); await rejected;
    const count = f.socket.sent.length;
    f.socket.open(); f.socket.frame({ type: 'ready' }); await tick();
    assert.equal(f.socket.sent.length, count);
  }
});

test('64KiB input capacity includes the in-flight batch; disposal rejects queued and held callers', async () => {
  let release; const gate = new Promise(resolve => { release = resolve }); const sent = [];
  const batcher = new TerminalInputBatcher(async data => { sent.push(data); await gate });
  const first = batcher.enqueue('x'.repeat(16384)); const rejected = [assert.rejects(first, /закрыт/)];
  await new Promise(resolve => setTimeout(resolve, 12));
  for (let index = 0; index < 3; index++) rejected.push(assert.rejects(batcher.enqueue('x'.repeat(16384)), /закрыт/));
  await assert.rejects(batcher.enqueue('z'), /64 KiB/);
  batcher.dispose(); await Promise.all(rejected); release(); await tick(); assert.equal(sent.length, 1);
});
