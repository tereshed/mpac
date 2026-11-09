import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import test from 'node:test'
import assert from 'node:assert/strict'
import { mobileAttentionEntries, mobileAttentionReasons, mobileBackgroundLabel, mobileFailedJobs, mobileHasBackground, mobileSessionMatches, mobileSessionTone, mobileSessions, mobileTime } from '../src/mobile-navigation.ts'

function activity(overrides = {}) {
  return { adapter: 'native', capabilities: { foreground: true, questions: true, approval: true, results: true, background: false }, phase: 'unknown', attention: null, result: { revision: 0, read_revision: 0, at: null }, background: { coverage: 'unsupported', observed_at: null, jobs: [] }, updated_at: null, source: 'native', ...overrides }
}
function session(id, observed, status = 'running', project_id = 'one') { return { id, title: id, project_id, agent: 'cli', activity: observed, status, started_at: 100, last_activity: 110 } }
const result = (revision = 1, read_revision = 0) => ({ revision, read_revision, at: 105 })
const job = (status = 'failed') => ({ id: 'job', name: 'Nightly checks', status, observed_at: 90, next_run_at: null })
test('session colors distinguish input, errors, work, results and history without changing state', () => {
  const items = [
    [session('input', activity({ attention: { kind: 'permission', certainty: 'requested', since: 1 }, result: result() })), 'attention'],
    [session('failure', activity(), 'failed'), 'error'],
    [session('work', activity({ phase: 'working', result: result() })), 'working'],
    [session('response', activity({ result: result() })), 'result'],
    [session('idle', activity({ phase: 'idle' })), 'idle'],
    [session('history', activity(), 'stopped'), 'history'],
    [session('unknown', undefined), 'unknown'],
  ]
  const before = structuredClone(items)
  for (const [item, tone] of items) assert.equal(mobileSessionTone(item), tone)
  assert.deepEqual(items, before)
})
test('a passive monitor stays purple with an unread result; native failure wins over background color', () => {
  const monitor = session('monitor', activity({ phase: 'idle', result: result(), background: { coverage: 'snapshot', observed_at: 90, jobs: [job('running')] } }))
  assert.equal(mobileSessionTone(monitor), 'background')
  assert.deepEqual(mobileAttentionEntries([monitor]), [])
  assert.equal(mobileSessionTone({ ...monitor, activity: { ...monitor.activity, background: { ...monitor.activity.background, jobs: [job('failed')] } } }), 'error')
  assert.equal(mobileSessionTone({ ...monitor, activity: { ...monitor.activity, attention: { kind: 'question', certainty: 'confirmed', since: 1 } } }), 'attention')
})

test('all-project action queue contains requests and failures, not ordinary unread responses', () => {
  const items = [session('unknown', undefined), session('result', activity({ result: result() }), 'running', 'two'), session('error', activity(), 'failed'), session('permission', activity({ attention: { kind: 'permission', certainty: 'requested', since: 20 } }), 'running', 'two'), session('question', activity({ attention: { kind: 'question', certainty: 'confirmed', since: 10 } }))]
  const copy = structuredClone(items)
  assert.deepEqual(mobileAttentionEntries(items).map(entry => entry.session.id), ['question', 'permission', 'error'])
  assert.equal(mobileAttentionEntries(items)[1].reasons[0].label, 'Запрошено разрешение')
  assert.match(mobileAttentionEntries(items)[1].reasons[0].detail, /ожидание ещё не подтверждено/)
  assert.deepEqual(items, copy)
})
test('one session carries independent pending, process, background and result reasons without duplicates', () => {
  const both = session('both', activity({ phase: 'working', attention: { kind: 'question', certainty: 'requested', since: 1 }, result: result(), background: { coverage: 'snapshot', observed_at: 3, jobs: [job()] } }), 'failed')
  const queue = mobileAttentionEntries([both])
  assert.equal(queue.length, 1)
  assert.equal(queue[0].priority, 0)
  assert.deepEqual(queue[0].reasons.map(reason => reason.kind), ['input', 'process', 'background', 'result'])
  assert.match(queue[0].reasons[2].label, /в снимке/)
  assert.match(queue[0].reasons[2].detail, /не проверка текущей работоспособности/)
})
test('only explicit failed jobs from native snapshot raise background attention', () => {
  const snapshot = session('snapshot', activity({ background: { coverage: 'snapshot', observed_at: 90, jobs: [job()] } }))
  assert.equal(mobileAttentionEntries([snapshot])[0].priority, 1)
  assert.equal(mobileFailedJobs(snapshot).length, 1)
  for (const coverage of ['unknown', 'awaiting_snapshot', 'unsupported']) {
    const uncertain = session(coverage, activity({ background: { coverage, observed_at: 90, jobs: [job()] } }))
    assert.equal(mobileFailedJobs(uncertain).length, 0)
    assert.equal(mobileAttentionEntries([uncertain]).length, 0)
  }
  for (const status of ['running', 'scheduled', 'completed', 'unknown']) assert.equal(mobileAttentionEntries([session(status, activity({ background: { coverage: 'snapshot', observed_at: 90, jobs: [job(status)] } }))]).length, 0)
})
test('unknown background coverage never displays invented zero tasks or successful heartbeat', () => {
  assert.match(mobileBackgroundLabel(session('legacy', undefined)), /неизвестен/)
  assert.equal(mobileHasBackground(session('legacy', undefined)), false)
  for (const coverage of ['unknown', 'awaiting_snapshot', 'unsupported']) {
    const unknown = session(coverage, activity({ background: { coverage, observed_at: null, jobs: [] } }))
    assert.doesNotMatch(mobileBackgroundLabel(unknown), /0 задач|успешно|работают/)
  }
  assert.match(mobileBackgroundLabel(session('empty', activity({ background: { coverage: 'snapshot', observed_at: 0, jobs: [] } }))), /Снимок фона: 0 задач/)
})
test('working filter requires model work observation, not a live PID', () => {
  for (const observed of [undefined, activity(), activity({ phase: 'idle' })]) assert.equal(mobileSessionMatches(session('live', observed), 'working'), false)
  assert.equal(mobileSessionMatches(session('native', activity({ phase: 'working' })), 'working'), true)
  assert.equal(mobileSessionMatches(session('ended', activity({ phase: 'working' }), 'stopped'), 'working'), false)
})
test('session filters preserve unread revision, input, explicit background support and stopped history', () => {
  const pending = session('pending', activity({ attention: { kind: 'input', certainty: 'confirmed', since: 1 }, result: result(3, 2) }))
  assert.equal(mobileSessionMatches(pending, 'input'), true)
  assert.equal(mobileSessionMatches(pending, 'results'), true)
  assert.equal(mobileSessionMatches(session('seen', activity({ result: result(3, 3) })), 'results'), false)
  assert.equal(mobileSessionMatches(session('await', activity({ background: { coverage: 'awaiting_snapshot', observed_at: null, jobs: [] } })), 'background'), true)
  assert.equal(mobileSessionMatches(session('unavailable', activity()), 'background'), false)
  for (const status of ['failed', 'interrupted', 'stopped', 'exited']) assert.equal(mobileSessionMatches(session(status, activity(), status), 'history'), true)
  assert.equal(mobileSessionMatches(session('live', activity()), 'history'), false)
})
test('project scope filters without mixing sessions and preserves live/history ordering', () => {
  const items = [session('other', activity(), 'running', 'two'), session('history', activity(), 'stopped'), session('live', activity()), session('need', activity({ result: result() }))]
  assert.deepEqual(mobileSessions(items, 'one', 'all').map(item => item.id), ['need', 'live', 'history'])
  assert.deepEqual(mobileSessions(items, null, 'results').map(item => item.id), ['need'])
  assert.equal(mobileSessions(items, null, 'all').length, 4)
})
test('oldest input sorts first and result-only sessions stay out of the action queue', () => {
  const pending = (id, since) => session(id, activity({ attention: { kind: 'question', certainty: 'requested', since } }))
  const items = [pending('late', 4), pending('early', 1), session('older-result', activity({ result: { ...result(), at: 10 } })), session('new-result', activity({ result: { ...result(), at: 15 } })), session('no-time', activity({ result: { ...result(), at: null } }))]
  assert.deepEqual(mobileAttentionEntries(items).map(entry => entry.session.id), ['early', 'late'])
})
test('a running monitor and its unread results remain visible without an action badge', () => {
  const monitor = session('disk-monitor', activity({ phase: 'idle', result: result(4, 0), background: { coverage: 'snapshot', observed_at: 90, jobs: [job('running')] } }))
  const before = structuredClone(monitor)
  assert.deepEqual(mobileAttentionEntries([monitor]), [])
  assert.deepEqual(mobileAttentionReasons(monitor), [])
  assert.equal(mobileSessionMatches(monitor, 'results'), true)
  assert.equal(mobileSessionMatches(monitor, 'background'), true)
  assert.deepEqual(mobileSessions([monitor], null, 'all').map(item => item.id), ['disk-monitor'])
  assert.deepEqual(monitor, before)
})
test('queue inspection and session filtering never acknowledge results or clear native requests', () => {
  const item = session('question', activity({ attention: { kind: 'question', certainty: 'confirmed', since: 10 }, result: result(2, 0) }))
  const copy = structuredClone(item)
  for (const incidental of [{ visible: true }, { input: 'yes\r' }, { seq: 1000 }]) {
    const next = { ...item, ...incidental }
    mobileAttentionEntries([next]); mobileSessions([next], null, 'all')
    assert.equal(mobileAttentionReasons(next).length, 2)
    assert.equal(next.activity.result.read_revision, 0)
  }
  assert.deepEqual(item, copy)
})
test('legacy needs_attention remains a generic action and unknown CLI alone remains absent', () => {
  const legacy = activity({ adapter: 'legacy', capabilities: { foreground: false, questions: false, approval: false, results: false, background: false } })
  assert.equal(mobileAttentionEntries([session('demo', legacy, 'needs_attention')])[0].reasons[0].label, 'Нужно внимание')
  assert.equal(mobileAttentionEntries([session('pid', legacy)]).length, 0)
})
test('event times preserve native seconds/milliseconds/ISO and unknown values', () => {
  assert.equal(mobileTime(1_700_000_000), mobileTime(1_700_000_000_000))
  assert.equal(mobileTime('2023-11-14T22:13:20Z'), 1_700_000_000_000)
  assert.equal(mobileTime(null), null)
  assert.equal(mobileTime('invalid'), null)
  assert.equal(mobileTime(Infinity), null)
  assert.equal(mobileTime(0), 0)
})


test('Working includes live custom commands without treating an open shell as model work', () => {
  assert.equal(mobileSessionMatches({ agent: 'command', status: 'running' }, 'working'), true)
  assert.equal(mobileSessionMatches({ agent: 'command', status: 'exited' }, 'working'), false)
  assert.equal(mobileSessionMatches({ agent: 'shell', status: 'running' }, 'working'), false)
})
