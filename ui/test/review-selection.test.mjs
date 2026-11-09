import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import test from 'node:test'
import assert from 'node:assert/strict'
import { selectedCodeLines, selectionAnchor, codeRangeLabel, commentRangeLabel, commentDelivery, batchCommentsForTarget, initialBatchSelection, initialBatchShowAll, sessionCoversComments, utf8Length } from '../src/review-selection.ts'
import { readReviewDraft, keepReviewDraft } from '../src/review-drafts.ts'

const lines = [
  { index: 0, old_line: 42, new_line: 42, kind: 'context', text: 'function title() {', hunk: 0 },
  { index: 1, old_line: 43, new_line: null, kind: 'deletion', text: 'old()', hunk: 0 },
  { index: 2, old_line: null, new_line: 43, kind: 'addition', text: '  new()', hunk: 0 },
  { index: 3, old_line: null, new_line: 44, kind: 'addition', text: '  wrap()', hunk: 0 },
  { index: 4, old_line: 44, new_line: 45, kind: 'context', text: '}', hunk: 0 },
]
const comment = { id: 'c1', project_id: 'p', root_id: 'r', path: 'src/title.ts', source: 'diff', snapshot: 's', text: 'Fix', revision: 2, anchor: { kind: 'range', start_row: 2, end_row: 3, side: 'new' }, old_start: null, old_end: null, new_start: 43, new_end: 44, excerpt: '  new()\n  wrap()', deleted: false }
const session = { id: 's1', project_id: 'p', root_id: 'r', additional_roots: [], status: 'running' }
const batch = (status, session_id = 's1', revision = 2) => ({ id: 'b', session_id, status, comments: [{ id: 'c1', revision }] })

test('reverse mouse range retains exact code lines and indentation', () => {
  const selection = { first: 3, last: 2, side: 'new' }
  assert.deepEqual(selectedCodeLines(lines, selection).map(line => line.text), ['  new()', '  wrap()'])
  assert.deepEqual(selectionAnchor(lines, selection, 7), { kind: 'range', block_id: 7, start_row: 2, end_row: 3, side: 'new' })
  assert.equal(codeRangeLabel(lines, selection), 'Новые строки 43–44')
})
test('deleted/new/mixed sides retain actual source numbers rather than displayed diff position', () => {
  assert.equal(codeRangeLabel(lines, { first: 1, last: 3, side: 'old' }), 'Старая строка 43')
  assert.equal(codeRangeLabel(lines, { first: 1, last: 3, side: 'both' }), 'Старая строка 43 · Новые строки 43–44')
  assert.equal(commentRangeLabel(comment), 'Новые строки 43–44')
  assert.equal(commentRangeLabel({ ...comment, anchor: { kind: 'file' } }), 'Весь файл')
})
test('range cannot bridge omitted hunk even if chosen side has no lines in one hunk', () => {
  const changed = lines.map(line => ({ ...line, hunk: line.index === 1 ? 4 : 0 }))
  assert.throws(() => selectionAnchor(changed, { first: 0, last: 3, side: 'new' }, 0), /пропущенные/)
})
test('empty and over-limit selections are rejected before request creation', () => {
  assert.throws(() => selectionAnchor(lines, { first: 1, last: 1, side: 'new' }), /Выделите/)
  const many = Array.from({ length: 201 }, (_, index) => ({ ...lines[0], index, new_line: index + 1 }))
  assert.throws(() => selectionAnchor(many, { first: 0, last: 200, side: 'new' }), /200/)
})
test('delivery status is specific to recipient and comment revision', () => {
  assert.equal(commentDelivery(comment, 's1', [batch('handed_off', 'other')]), 'new')
  assert.equal(commentDelivery(comment, 's1', [batch('handed_off', 's1', 1)]), 'new')
  assert.equal(commentDelivery(comment, 's1', [batch('handed_off')]), 'handed_off')
})
test('prepared and uncertain drafts do not claim handoff', () => {
  assert.equal(commentDelivery(comment, 's1', [batch('prepared')]), 'prepared')
  assert.equal(commentDelivery(comment, 's1', [batch('preparing')]), 'uncertain')
  assert.equal(commentDelivery(comment, 's1', [batch('uncertain')]), 'uncertain')
})
test('authoritative handoff ledger survives paginated recent batches', () => {
  const sent = { ...comment, deliveries: [{ session_id: 's1', revision: 2, batch_id: 'older', status: 'handed_off', at: 1 }] }
  assert.equal(commentDelivery(sent, 's1', []), 'handed_off')
  assert.equal(commentDelivery({ ...sent, revision: 3 }, 's1', []), 'new')
})
test('batch recipients must cover every root and remain in the project with a live process', () => {
  assert.equal(sessionCoversComments(session, [comment], 'p'), true)
  assert.equal(sessionCoversComments(session, [comment, { ...comment, root_id: 'extra' }], 'p'), false)
  assert.equal(sessionCoversComments({ ...session, additional_roots: [{ id: 'extra' }] }, [comment, { ...comment, root_id: 'extra' }], 'p'), true)
  assert.equal(sessionCoversComments({ ...session, status: 'stopped' }, [comment], 'p'), false)
  assert.equal(sessionCoversComments({ ...session, project_id: 'other' }, [comment], 'p'), false)
})
test('UTF8 limits count Cyrillic and emoji bytes rather than characters', () => {
  assert.equal(utf8Length('я🙂'), 6)
  assert.equal(utf8Length('я'.repeat(2001)), 4002)
})
test('memory-only drafts retain a frozen anchor and are isolated by authenticated context', () => {
  const editor = { context: 'tokenA/project/root/file', snapshot: 'old', anchor: { kind: 'range', start_row: 2, end_row: 3, side: 'new' } }
  keepReviewDraft('tokenA/project/root/file', editor, 'Unsaved note')
  assert.deepEqual(readReviewDraft('tokenA/project/root/file'), { editor, text: 'Unsaved note' })
  assert.deepEqual(readReviewDraft('tokenB/project/root/file'), { editor: null, text: '' })
  keepReviewDraft('tokenA/project/root/file', null, '')
  assert.deepEqual(readReviewDraft('tokenA/project/root/file'), { editor: null, text: '' })
})
test('full batch scope exposes previously excluded comments when recipient or delivery filter changes', () => {
  const forwarded = { ...comment, id: 'forwarded', deliveries: [{ session_id: 'claude', revision: 2, batch_id: 'previous', status: 'handed_off', at: 1 }] }
  const unselected = { ...comment, id: 'unselected' }
  const removed = { ...comment, id: 'removed', deleted: true }
  const scope = [comment, forwarded, unselected, removed]
  const initial = initialBatchSelection(scope, ['c1'])
  assert.deepEqual([...initial], ['c1'])
  assert.deepEqual(batchCommentsForTarget(scope, 'claude', false).map(item => item.id), ['c1', 'unselected'])
  assert.deepEqual(batchCommentsForTarget(scope, 'claude', true).map(item => item.id), ['c1', 'forwarded', 'unselected'])
  assert.deepEqual(batchCommentsForTarget(scope, 'codex', false).map(item => item.id), ['c1', 'forwarded', 'unselected'])
  assert.deepEqual(batchCommentsForTarget(scope, '__new__', false).map(item => item.id), ['c1', 'forwarded', 'unselected'])
  // Revealing eligible candidates must not silently select them.
  assert.deepEqual([...initial], ['c1'])
  const selectAll = initialBatchSelection(scope, batchCommentsForTarget(scope, 'codex', false).map(item => item.id))
  assert.deepEqual([...selectAll], ['c1', 'forwarded', 'unselected'])
})
test('an explicitly empty batch selection stays empty and deleted/out-of-scope IDs cannot return', () => {
  assert.deepEqual([...initialBatchSelection([comment], [])], [])
  assert.deepEqual([...initialBatchSelection([comment], ['elsewhere'])], [])
  assert.deepEqual([...initialBatchSelection([{ ...comment, deleted: true }], ['c1'])], [])
})
test('explicitly selected handed-off versions remain visible when a batch dialog opens', () => {
  const forwarded = { ...comment, id: 'forwarded', deliveries: [{ session_id: 'claude', revision: 2, batch_id: 'previous', status: 'handed_off', at: 1 }] }
  const scope = [comment, forwarded]
  const selectedIds = ['forwarded']
  const selected = initialBatchSelection(scope, selectedIds)
  const showAll = initialBatchShowAll(scope, selectedIds, 'claude')
  assert.equal(showAll, true)
  const shown = new Set(batchCommentsForTarget(scope, 'claude', showAll).map(item => item.id))
  assert.deepEqual([...selected], ['forwarded'])
  assert.equal([...selected].every(id => shown.has(id)), true)
  assert.equal(initialBatchShowAll(scope, ['c1'], 'claude'), false)
  assert.equal(initialBatchShowAll(scope, [], 'claude'), false)
  assert.equal(initialBatchShowAll(scope, selectedIds, 'codex'), false)
  assert.equal(initialBatchShowAll([{ ...forwarded, revision: 3 }], selectedIds, 'claude'), false)
})
