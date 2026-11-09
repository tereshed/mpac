import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import test from 'node:test'
import assert from 'node:assert/strict'
import { findTerminalFileLinks, findTerminalFileLinksInRows, resolveTerminalFileLink, MAX_TERMINAL_LINK_ROWS } from '../src/terminal-file-links.ts'

const main = { id: 'main', path: '/Users/me/project' }
const other = { id: 'shared', path: '/Users/me/shared' }
const roots = [main, other]
const locate = reference => resolveTerminalFileLink(reference, main, roots)

test('relative file paths and anchors use the session cwd, never a toolbar root', () => {
  assert.deepEqual(locate('src/studio/Cover.tsx:44:7'), { rootId: 'main', path: 'src/studio/Cover.tsx', line: 44, column: 7 })
  assert.deepEqual(locate('./README.md#L12-L18'), { rootId: 'main', path: 'README.md', line: 12, endLine: 18 })
  assert.deepEqual(locate('../shared/docs/guide.md:9'), { rootId: 'shared', path: 'docs/guide.md', line: 9 })
  assert.deepEqual(locate('src/deep/../file.ts'), { rootId: 'main', path: 'src/file.ts' })
})
test('absolute references choose the longest granted root by path boundary', () => {
  const nested = { id: 'nested', path: '/Users/me/project/packages/ui' }
  assert.deepEqual(resolveTerminalFileLink('/Users/me/project/packages/ui/src/界.ts:2', main, [...roots, nested]), { rootId: 'nested', path: 'src/界.ts', line: 2 })
  assert.throws(() => locate('/Users/me/project-foreign/secrets.md'), /вне папок/)
})
test('file URI decoding retains local host rules and line anchors', () => {
  assert.deepEqual(locate('file://localhost/Users/me/project/docs/my%20guide.md#L7-L11'), { rootId: 'main', path: 'docs/my guide.md', line: 7, endLine: 11 })
  assert.deepEqual(locate('file:///Users/me/project/src/файл.ts:12:3'), { rootId: 'main', path: 'src/файл.ts', line: 12, column: 3 })
  assert.deepEqual(locate('file:///Users/me/project/literal%3A12'), { rootId: 'main', path: 'literal:12' })
  assert.throws(() => locate('file://server/share/code.ts'), /локальные/)
  assert.throws(() => locate('file:///Users/me/project/code.ts?run=x'), /локальные/)
})
test('quoted spaces and absolute root trailing spaces remain literal', () => {
  assert.deepEqual(locate('"docs/my guide.md":12'), { rootId: 'main', path: 'docs/my guide.md', line: 12 })
  assert.deepEqual(locate("'docs/my guide.md:12:4'"), { rootId: 'main', path: 'docs/my guide.md', line: 12, column: 4 })
  const literal = { id: 'space', path: '/Users/me/project ' }
  assert.deepEqual(resolveTerminalFileLink('src/file.ts', literal, [literal]), { rootId: 'space', path: 'src/file.ts' })
})
test('Windows paths normalize separators and ASCII case only on Windows roots', () => {
  const cwd = { id: 'win', path: 'C:\\Work\\Project' }
  assert.deepEqual(resolveTerminalFileLink('.\\src\\Foo.ts:8:2', cwd, [cwd]), { rootId: 'win', path: 'src/Foo.ts', line: 8, column: 2 })
  assert.deepEqual(resolveTerminalFileLink('file:///c:/work/project/src/Foo.ts#L9', cwd, [cwd]), { rootId: 'win', path: 'src/Foo.ts', line: 9 })
  assert.throws(() => resolveTerminalFileLink('C:\\Work\\ProjectExtra\\secret.ts', cwd, [cwd]), /вне папок/)
  assert.throws(() => resolveTerminalFileLink('\\\\server\\share\\file.ts', cwd, [cwd]), /Сетевые/)
})
test('POSIX backslashes are literal filename characters', () => {
  assert.deepEqual(locate('src\\file.ts'), { rootId: 'main', path: 'src\\file.ts' })
})
test('path traversal, remote schemes, guessed homes and controls cannot escape roots', () => {
  for (const reference of ['../../private/secret.md', '/etc/passwd', '../project-sibling/code.ts']) assert.throws(() => locate(reference), /вне папок|пределы/)
  for (const reference of ['https://example.com/file.ts', 'ssh://host/file.ts', 'javascript:alert(1)']) assert.throws(() => locate(reference), /локальные/)
  assert.throws(() => locate('~/project/file.ts'), /полный путь/)
  assert.throws(() => locate('src/a\u0000.ts'), /символы/)
  assert.throws(() => locate('src/\u202efile.ts'), /символы/)
  assert.throws(() => locate('src/a.ts:0'), /номер/)
  assert.throws(() => locate('src/a.ts#L9-L2'), /Конец диапазона/)
})
test('finder recognizes relative, absolute, markdown and quoted links without remote URLs', () => {
  const text = 'Changed src/a.ts:12:3 and ./docs/guide.md#L2-L5; [read](docs/my guide.md:4) "src/my file.ts":7 https://remote/file.ts'
  assert.deepEqual(findTerminalFileLinks(text).map(link => link.text), ['src/a.ts:12:3', './docs/guide.md#L2-L5', 'docs/my guide.md:4', '"src/my file.ts":7'])
  for (const link of findTerminalFileLinks(text)) assert.equal(text.slice(link.start, link.end), link.text)
})
test('finder uses UTF16 offsets and retains filename unicode', () => {
  const text = '🙂 界 — src/файл.ts:4 and README.md.'
  const links = findTerminalFileLinks(text)
  assert.equal(links[0].start, text.indexOf('src/'))
  assert.deepEqual(links.map(link => link.text), ['src/файл.ts:4', 'README.md'])
})
test('finder accepts file URIs, parent-relative and extensionless paths but not malformed line refs', () => {
  assert.deepEqual(findTerminalFileLinks('file:///Users/me/project/a.ts#L3 ../shared/src/main ./scripts/start Dockerfile src/a.ts:0').map(link => link.text), ['file:///Users/me/project/a.ts#L3', '../shared/src/main', './scripts/start', 'Dockerfile'])
  assert.equal(findTerminalFileLinks('x'.repeat(9000)).length, 0)
})

function cell(chars, width = 1) { return { getChars: () => chars, getWidth: () => width } }
function row(cells, isWrapped = false) { return { isWrapped, length: cells.length, getCell: index => cells[index] } }
function ascii(text, wrapped = false) { return row([...text].map(char => cell(char)), wrapped) }
test('soft-wrapped links map to inclusive 1-based xterm cell coordinates', () => {
  const first = 'src/long/fi', second = 'le.ts:12:3'
  const links = findTerminalFileLinksInRows([ascii(first), ascii(second, true)], 9)
  assert.deepEqual(links, [{ text: first + second, range: { start: { x: 1, y: 9 }, end: { x: second.length, y: 10 } } }])
})
test('wide Unicode and combining clusters before links do not shift cell hit targets', () => {
  const wide = row([cell('界', 2), cell('', 0), cell(' '), ...[...'src/a.ts'].map(char => cell(char))])
  assert.deepEqual(findTerminalFileLinksInRows([wide], 1)[0].range, { start: { x: 4, y: 1 }, end: { x: 11, y: 1 } })
  const emoji = row([cell('🙂', 2), cell('', 0), cell(' '), ...[...'a.ts'].map(char => cell(char))])
  assert.deepEqual(findTerminalFileLinksInRows([emoji], 2)[0].range, { start: { x: 4, y: 2 }, end: { x: 7, y: 2 } })
  const combining = row([cell('e\u0301'), cell(' '), ...[...'a.ts'].map(char => cell(char))])
  assert.deepEqual(findTerminalFileLinksInRows([combining], 3)[0].range, { start: { x: 3, y: 3 }, end: { x: 6, y: 3 } })
})
test('wide characters inside a wrapped path skip null wrap padding without losing offsets', () => {
  const first = row([...[...'src/'].map(char => cell(char)), cell('')])
  const second = row([cell('文', 2), cell('', 0), cell('件', 2), cell('', 0), ...[...'.ts:3'].map(char => cell(char))], true)
  const links = findTerminalFileLinksInRows([first, second], 4)
  assert.deepEqual(links, [{ text: 'src/文件.ts:3', range: { start: { x: 1, y: 4 }, end: { x: 9, y: 5 } } }])
})
test('ordinary printed spaces and separate terminal rows never merge into a false file link', () => {
  const links = findTerminalFileLinksInRows([ascii('prefix '), ascii('a.ts', true)], 1)
  assert.deepEqual(links[0], { text: 'a.ts', range: { start: { x: 1, y: 2 }, end: { x: 4, y: 2 } } })
  assert.deepEqual(findTerminalFileLinksInRows([ascii('src/'), ascii('a.ts')], 1), [])
})
test('truncated prefix or oversized wrapped line fails closed', () => {
  assert.deepEqual(findTerminalFileLinksInRows([ascii('prefix/a.ts', true)], 1), [])
  assert.deepEqual(findTerminalFileLinksInRows(Array.from({ length: MAX_TERMINAL_LINK_ROWS + 1 }, (_, i) => ascii('a', i > 0)), 1), [])
})
