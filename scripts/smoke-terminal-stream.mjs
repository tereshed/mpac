#!/usr/bin/env node
// Own loopback fixture only. Fake Claude raw echo; no models or main-space I/O.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { TerminalStream } from '../ui/src/terminal-stream.ts';
const folder = path.resolve(process.argv[2] || '.mpac/streaming-preview');
const owned = JSON.parse(fs.readFileSync(path.join(folder, 'owned.json')));
const launch = JSON.parse(fs.readFileSync(path.join(owned.data, 'launch.json')));
assert.equal(launch.pid, owned.pid);
const token = fs.readFileSync(path.join(owned.data, 'auth-token'), 'utf8');
const base = `http://127.0.0.1:${owned.port}`;
async function api(route, body) {
  const response = await fetch(base + '/api' + route, { method: body === undefined ? 'GET' : 'POST', headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
  assert.equal(response.status, 200); return response.json();
}
const state = await api('/state'); assert.equal(state.capabilities.terminal_stream, true);
const project = state.projects[0];
async function session(title) { return api('/sessions', { project_id: project.id, root_id: project.roots[0].id, agent: 'claude', title }) }
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(predicate, ms = 5000) { const deadline = performance.now() + ms; while (!predicate()) { assert.ok(performance.now() < deadline, 'timed out waiting for own terminal output'); await sleep(2) } }
const live = await session('Streaming latency QA');
let text = '', cursor = 0; const failures = [];
const connect = after => new TerminalStream({ url: `ws://127.0.0.1:${owned.port}/api/sessions/${live.id}/terminal`, token, sessionId: live.id, after,
  onOutput: async output => { if (output.reset) text = ''; text += Buffer.from(output.data_base64, 'base64').toString(); cursor = output.seq }, onDisconnect: error => failures.push(error.message), onEnd: () => {} });
let stream = connect(0); await stream.ready;
await until(() => text.includes('STREAM QA'));
const samples = [];
for (let index = 0; index < 30; index++) {
  const marker = `~WS-${index}-é文~`; const started = performance.now();
  await stream.input(marker); await until(() => text.includes(marker)); samples.push(performance.now() - started);
}
// Close and resume output at exactly the rendered byte without replaying input.
stream.close(); await sleep(20); const before = text; stream = connect(cursor); await stream.ready;
await sleep(30); assert.equal(text, before); await stream.input('~RESUME~'); await until(() => text.endsWith('~RESUME~'));
assert.equal(text.split('~RESUME~').length - 1, 1);
await stream.resize(77, 19); await until(() => text.includes('[SIZE 77x19]'));
// Keep this session running for browser/native UI QA.
stream.close(); assert.deepEqual(failures, []);
const old = await session('Previous polling schedule QA');
let oldText = '', oldSeq = 0, stopped = false, pollError;
async function poll() { try { const output = await api(`/sessions/${old.id}/output?after=${oldSeq}`); oldText += Buffer.from(output.data_base64, 'base64').toString(); oldSeq = output.seq } catch (error) { pollError = error } if (!stopped) setTimeout(poll, 400) }
void poll(); await until(() => oldText.includes('STREAM QA'));
const former = [];
for (let index = 0; index < 12; index++) {
  await sleep((index * 53) % 240); const marker = `~POLL-${index}~`, started = performance.now();
  await api(`/sessions/${old.id}/input`, { data: marker }); await until(() => oldText.includes(marker)); former.push(performance.now() - started);
}
stopped = true; assert.equal(pollError, undefined); await api(`/sessions/${old.id}/stop`, {});
const stats = values => { const sorted = [...values].sort((a, b) => a - b); return { samples: sorted.length, median_ms: +sorted[Math.floor(sorted.length * .5)].toFixed(2), p95_ms: +sorted[Math.floor(sorted.length * .95)].toFixed(2), max_ms: +sorted.at(-1).toFixed(2) } };
const report = { scope: 'Isolated Rust daemon + real PTY + owned raw-echo fake Claude; no models. Former schedule emulated against same fixture.', checks: ['first-frame auth without credential URL', 'ordered UTF8 and key echo', 'output-only reconnect without duplicated bytes', 'resize acknowledged and SIGWINCH observed', 'viewer disconnect preserves CLI'], websocket: stats(samples), former_400ms_polling: stats(former), ui_session: live.id, daemon_pid: owned.pid, port: owned.port };
fs.writeFileSync(path.join(folder, 'stream-smoke.json'), JSON.stringify(report, null, 2) + '\n'); console.log(JSON.stringify(report));
