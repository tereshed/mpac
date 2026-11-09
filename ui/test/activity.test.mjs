import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import test from 'node:test'
import assert from 'node:assert/strict'
import { activitySummary, attentionPriority, backgroundSummary, backgroundInventoryLabel, passiveBackground, toolCallLabel, unreadResult, activityTime } from '../src/activity.ts'
import { needsAttention, orderedSessions } from '../src/session-management.ts'

function activity(overrides = {}) {
  return { adapter: 'claude', capabilities: { foreground: true, questions: true, approval: true, results: true, background: true }, phase: 'unknown', attention: null, result: { revision: 0, read_revision: 0, at: null }, background: { coverage: 'awaiting_snapshot', observed_at: null, jobs: [] }, updated_at: null, source: 'native', ...overrides }
}
function session(id, observed, status = 'running') { return { id, status, activity: observed, started_at: 100 } }

test('a live CLI alone never establishes model work or confirmed idle', () => {
  const legacy = session('legacy', undefined)
  assert.equal(activitySummary(legacy).tone, 'unknown')
  assert.match(activitySummary(legacy).label, /неизвестен/)
  assert.match(activitySummary(legacy).detail, /не подтверждает/)
  assert.equal(activitySummary(session('unknown', activity())).tone, 'unknown')
  assert.equal(activitySummary(session('work', activity({ phase: 'working' }))).tone, 'working')
  assert.equal(activitySummary(session('idle', activity({ phase: 'idle' }))).tone, 'idle')
})
test('default legacy metadata preserves coarse demo attention without inventing a question', () => {
  const legacy = session('demo', activity({ adapter: 'legacy', capabilities: { foreground: false, questions: false, approval: false, results: false, background: false } }), 'needs_attention')
  assert.equal(needsAttention(legacy), true)
  assert.equal(attentionPriority(legacy), 0)
  assert.equal(activitySummary(legacy).label, 'Нужно внимание')
  assert.equal(activitySummary(session('old', undefined, 'needs_attention')).label, 'Нужно внимание')
  assert.equal(needsAttention(session('native', activity(), 'needs_attention')), false)
  assert.equal(needsAttention(session('live', legacy.activity)), false)
})
test('requested questions and permissions do not claim confirmed waiting', () => {
  for (const kind of ['question', 'permission', 'input']) {
    const requested = session(kind, activity({ attention: { kind, certainty: 'requested', since: 100 } }))
    assert.match(activitySummary(requested).label, /Запрошен/)
    assert.match(activitySummary(requested).detail, /Проверьте терминал/)
    const confirmed = session(kind, activity({ attention: { kind, certainty: 'confirmed', since: 100 } }))
    assert.match(activitySummary(confirmed).label, /Ждёт/)
  }
})
test('pending question wins over unread result while result remains durable', () => {
  const pending = session('pending', activity({ phase: 'unknown', attention: { kind: 'question', certainty: 'requested', since: 100 }, result: { revision: 5, read_revision: 4, at: 100 } }))
  assert.equal(activitySummary(pending).tone, 'attention')
  assert.equal(unreadResult(pending), true)
  assert.equal(attentionPriority(pending), 0)
  const copy = structuredClone(pending)
  activitySummary(pending); needsAttention(pending); orderedSessions([pending])
  assert.deepEqual(pending, copy)
})
test('acknowledging one result revision never acknowledges a later one', () => {
  assert.equal(unreadResult(session('read', activity({ result: { revision: 3, read_revision: 3, at: 1 } }))), false)
  assert.equal(unreadResult(session('new', activity({ result: { revision: 4, read_revision: 3, at: 1 } }))), true)
  for (const revision of [0, -1, NaN, Infinity, 1.5]) assert.equal(unreadResult(session('invalid', activity({ result: { revision, read_revision: 0, at: null } }))), false)
})
test('reading result leaves foreground phase, pending question and background jobs intact', () => {
  const before = session('both', activity({ phase: 'working', attention: { kind: 'permission', certainty: 'confirmed', since: 10 }, result: { revision: 9, read_revision: 8, at: 11 }, background: { coverage: 'snapshot', observed_at: 12, jobs: [{ id: 'bg', status: 'running' }] } }))
  const after = { ...before, activity: { ...before.activity, result: { ...before.activity.result, read_revision: 9 } } }
  assert.equal(unreadResult(after), false)
  assert.equal(activitySummary(after).label, 'Ждёт разрешения')
  assert.equal(after.activity.phase, 'working')
  assert.deepEqual(after.activity.background, before.activity.background)
})
test('opening terminal, receiving output or pressing a key does not clear a result or question', () => {
  const observed = activity({ attention: { kind: 'question', certainty: 'confirmed', since: 10 }, result: { revision: 2, read_revision: 0, at: 11 } })
  const before = session('cli', observed)
  for (const incidental of [{ visible: true }, { seq: 999, last_activity: 999 }, { input: 'yes\r' }]) {
    const next = { ...before, ...incidental }
    assert.equal(unreadResult(next), true)
    assert.equal(activitySummary(next).label, 'Ждёт ответа')
    assert.equal(needsAttention(next), true)
  }
})
test('unread results sort before ordinary sessions but are not requests for action', () => {
  const items = [session('history', activity(), 'exited'), session('live', activity({ phase: 'working' })), session('unread', activity({ result: { revision: 1, read_revision: 0, at: 1 } })), session('failed', activity(), 'failed'), session('question', activity({ attention: { kind: 'question', certainty: 'requested', since: 1 } }))]
  assert.deepEqual(orderedSessions(items).map(item => item.id), ['question', 'failed', 'unread', 'live', 'history'])
  assert.deepEqual(orderedSessions(items, true).map(item => item.id), ['question', 'failed'])
  assert.equal(needsAttention(items.find(item => item.id === 'unread')), false)
  assert.equal(needsAttention(session('demo', undefined, 'needs_attention')), true)
})
test('a healthy monitor with an unread response is passive background work and never needs action', () => {
  const item = session('monitor', activity({ phase: 'idle', result: { revision: 4, read_revision: 0, at: 100 }, background: { coverage: 'snapshot', observed_at: 100, jobs: [{ id: 'monitor', name: 'Disk space', kind: 'shell', status: 'running', observed_at: 100, next_run_at: null }] } }))
  const copy = structuredClone(item)
  assert.equal(activitySummary(item).label, 'Фоновая работа · снимок')
  assert.equal(passiveBackground(item), true)
  assert.equal(needsAttention(item), false)
  assert.equal(unreadResult(item), true)
  assert.deepEqual(item, copy)
  for (const kind of ['question', 'permission']) {
    const pending = { ...item, activity: { ...item.activity, attention: { kind, certainty: 'confirmed', since: 101 } } }
    assert.equal(needsAttention(pending), true)
    assert.equal(passiveBackground(pending), false)
    assert.equal(activitySummary(pending).tone, 'attention')
  }
  const failed = { ...item, activity: { ...item.activity, background: { ...item.activity.background, jobs: [{ ...item.activity.background.jobs[0], status: 'failed' }] } } }
  assert.equal(needsAttention(failed), true)
  assert.equal(passiveBackground(failed), false)
})
test('schedules are passive and incomplete observations never establish background work', () => {
  const item = session('schedule', activity({ background: { coverage: 'snapshot', observed_at: 100, jobs: [{ id: 'cron', kind: 'cron', status: 'scheduled' }] } }))
  assert.equal(activitySummary(item).label, 'По расписанию · снимок')
  assert.equal(needsAttention(item), false)
  for (const coverage of ['unknown', 'awaiting_snapshot', 'unsupported']) {
    const unknown = { ...item, activity: { ...item.activity, background: { ...item.activity.background, coverage } } }
    assert.equal(passiveBackground(unknown), false)
    assert.equal(activitySummary(unknown).tone, 'unknown')
  }
  const working = { ...item, activity: { ...item.activity, phase: 'working', result: { revision: 2, read_revision: 1, at: 100 } } }
  assert.equal(activitySummary(working).label, 'Агент выполняет задачу')
  assert.equal(passiveBackground(working), false)
})
test('missing and pending background snapshots never mean zero jobs or healthy state', () => {
  assert.equal(backgroundSummary().showJobs, false)
  assert.match(backgroundSummary().detail, /не означает, что задач нет/)
  for (const coverage of ['unsupported', 'awaiting_snapshot', 'unknown']) {
    const info = backgroundSummary(activity({ background: { coverage, observed_at: 9999, jobs: [] } }))
    assert.equal(info.showJobs, false)
    assert.doesNotMatch(info.label, /работают|успешно|нет задач/i)
  }
})
test('native snapshot is an observation, not a health check or fabricated schedule', () => {
  const info = backgroundSummary(activity({ background: { coverage: 'snapshot', observed_at: 100, jobs: [] } }))
  assert.equal(info.showJobs, true)
  assert.match(info.detail, /не является подтверждением/)
  assert.match(info.detail, /heartbeat/)
  assert.equal(activityTime(null), 'неизвестно')
  assert.equal(activityTime(NaN), 'неизвестно')
  assert.notEqual(activityTime(0), 'неизвестно')
  assert.equal(activityTime(1_700_000_000), activityTime(1_700_000_000_000))
})
test('tool calls are native observations, preserve raw names and never override a question', () => {
  const observed = activity({ phase: 'working', tools: [{ id: 'one', name: 'Read', started_at: 10 }, { id: 'two', name: 'mcp__monitor__status', started_at: 11 }], tools_observed_at: 11, attention: { kind: 'permission', certainty: 'requested', since: 10 } })
  const item = session('calls', observed), copy = structuredClone(item)
  assert.equal(toolCallLabel(item), 'Вызов инструмента: Read, mcp__monitor__status')
  assert.equal(activitySummary(item).tone, 'attention')
  assert.deepEqual(item, copy)
  assert.equal(toolCallLabel(session('legacy', activity())), '')
  assert.equal(toolCallLabel(session('ended', observed, 'exited')), '')
  assert.equal(toolCallLabel(session('no-observation', { ...observed, tools_observed_at: null })), '')
  assert.equal(toolCallLabel(session('zero-clock', { ...observed, tools_observed_at: 0 })), 'Вызов инструмента: Read, mcp__monitor__status')
})
test('known background jobs remain visible as partial observations without inventing monitor health', () => {
  const jobs = [{ id: 'monitor', name: 'Build monitor', kind: 'bash', status: 'failed', observed_at: 10, next_run_at: null }, { id: 'cron', name: '*/15 * * * *', kind: 'cron', status: 'scheduled', observed_at: 11, next_run_at: 100 }]
  const item = session('partial', activity({ background: { coverage: 'unknown', observed_at: 10, jobs } })), before = structuredClone(item)
  assert.equal(backgroundSummary(item.activity).showJobs, true)
  assert.match(backgroundInventoryLabel(item), /Фон: 2 · по расписанию: 1 · ошибок: 1 · неполные данные/)
  assert.doesNotMatch(backgroundInventoryLabel(item), /успешно|работает|heartbeat/)
  assert.deepEqual(item, before)
  const snapshot = session('snapshot', { ...item.activity, background: { coverage: 'snapshot', observed_at: 11, jobs } })
  assert.match(backgroundInventoryLabel(snapshot), /· снимок$/)
  assert.equal(needsAttention(snapshot), true)
  assert.equal(attentionPriority(snapshot), 1)
  assert.equal(activitySummary(snapshot).label, 'Ошибка фоновой задачи в снимке')
  assert.equal(needsAttention(item), false, 'an incomplete observation cannot invent a current failed monitor')
  assert.equal(backgroundSummary({ ...item.activity, background: { coverage: 'awaiting_snapshot', observed_at: null, jobs } }).showJobs, true)
  assert.equal(backgroundInventoryLabel(session('unsupported', activity({ background: { coverage: 'unsupported', observed_at: null, jobs: [] } }))), '')
})

test('plain shells and commands describe process state without inventing AI work', () => {
  const shell = { id: 'shell', agent: 'shell', status: 'running' }
  assert.equal(activitySummary(shell).tone, 'idle')
  assert.equal(activitySummary(shell).label, 'Терминал открыт')
  assert.equal(needsAttention(shell), false)
  assert.equal(activitySummary({ ...shell, agent: 'command' }).tone, 'working')
  assert.equal(activitySummary({ ...shell, agent: 'command', status: 'exited', exit_code: 0 }).tone, 'stopped')
  assert.equal(activitySummary({ ...shell, agent: 'command', status: 'failed', exit_code: 7 }).tone, 'attention')
})
