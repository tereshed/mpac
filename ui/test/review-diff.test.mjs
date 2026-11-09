import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import test from 'node:test'
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { chmodSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { reviewFileDiffs } from '../src/reviewDiff.ts'

function fixture(t, files) {
  const dir = mkdtempSync(join(tmpdir(), 'mpac-file-diff-'))
  t.after(() => rmSync(dir, { recursive: true, force: true }))
  const git = (...args) => execFileSync('git', [
    '--no-pager', '-c', 'core.hooksPath=.mpac-no-hooks', '-c', 'core.quotePath=true',
    '-c', 'core.filemode=true', '-c', 'diff.noprefix=false', '-c', 'diff.mnemonicprefix=false',
    '-c', 'user.name=MPAC Test', '-c', 'user.email=mpac@example.invalid', '-c', 'commit.gpgsign=false', ...args,
  ], { cwd: dir, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
  const write = (path, content) => { mkdirSync(dirname(join(dir, path)), { recursive: true }); writeFileSync(join(dir, path), content) }
  git('init', '--quiet')
  for (const [path, content] of Object.entries(files)) write(path, content)
  git('add', '--all'); git('commit', '--quiet', '-m', 'fixture')
  return { dir, git, write, diff: (...args) => git('diff', '--binary', '--full-index', '--no-ext-diff', '--no-textconv', ...args) }
}

const manifest = (...paths) => paths.map(path => ({ path, status: ' M' }))

function synthetic(path, body = '+new content\n', count = 1) {
  const quoted = JSON.stringify('b/' + path)
  return `diff --git ${quoted} ${quoted}\nnew file mode 100644\n--- /dev/null\n+++ ${quoted}\n@@ -0,0 +1,${count} @@\n${body}`
}

test('real staged and working changes for the same file retain both sections', t => {
  const f = fixture(t, { 'notes.md': 'base\n', 'other.md': 'other base\n' })
  f.write('notes.md', 'staged\n'); f.git('add', '--', 'notes.md')
  f.write('notes.md', 'working\n'); f.write('other.md', 'other working\n')
  const raw = '# Staged changes\n\n' + f.diff('--cached') + '\n# Working tree changes\n\n' + f.diff()
  const views = reviewFileDiffs(raw, manifest('notes.md', 'other.md'))
  assert.equal(views.size, 2)
  assert.match(views.get('notes.md'), /# Staged changes[\s\S]*\+staged[\s\S]*# Working tree changes[\s\S]*\+working/)
  assert.doesNotMatch(views.get('notes.md'), /other working/)
  assert.doesNotMatch(views.get('other.md'), /# Staged changes/)
})

test('real C-quoted UTF-8 octal, tabs, quotes, backslashes and spaces map exactly', t => {
  const paths = ['кириллица.md', 'tab\tname.md', 'quote"name.md', 'literal\\name.md', 'space name.md', 'has b/path.md']
  const f = fixture(t, Object.fromEntries(paths.map((path, i) => [path, `before ${i}\n`])))
  for (const [i, path] of paths.entries()) f.write(path, `after ${i}\n`)
  const views = reviewFileDiffs(f.diff(), manifest(...paths))
  assert.equal(views.size, paths.length)
  for (const [i, path] of paths.entries()) assert.match(views.get(path), new RegExp(`\\+after ${i}`))
})

test('core.quotePath=false Unicode and ambiguous literal b/ separators remain exact', t => {
  const paths = ['folder b/other b/name.md', 'other b/name.md', 'name.md', 'план 🦀.md']
  const f = fixture(t, Object.fromEntries(paths.map((path, i) => [path, `before ${i}\n`])))
  for (const [i, path] of paths.entries()) f.write(path, `selected ${i}\n`)
  const raw = f.git('-c', 'core.quotePath=false', 'diff', '--no-ext-diff', '--no-textconv')
  const views = reviewFileDiffs(raw, manifest(...paths))
  assert.equal(views.size, paths.length)
  for (const [i, path] of paths.entries()) {
    assert.match(views.get(path), new RegExp(`\\+selected ${i}`))
    for (let other = 0; other < paths.length; other++) if (other !== i) assert.doesNotMatch(views.get(path), new RegExp(`\\+selected ${other}`))
  }
})

test('real rename-only and deleted-file diffs map to destination and source respectively', t => {
  const f = fixture(t, { 'old name.md': 'unchanged\n', 'removed.md': 'remove this\n' })
  renameSync(join(f.dir, 'old name.md'), join(f.dir, 'new name.md'))
  rmSync(join(f.dir, 'removed.md')); f.git('add', '--all')
  const views = reviewFileDiffs(f.diff('--cached'), manifest('new name.md', 'removed.md'))
  assert.equal(views.size, 2)
  assert.match(views.get('new name.md'), /rename from old name.md\nrename to new name.md/)
  assert.match(views.get('removed.md'), /deleted file mode/)
  assert.equal(views.has('old name.md'), false)
})

test('real copy-only diff maps to copied destination even when source is also listed', t => {
  const f = fixture(t, { 'original.md': 'copy contents\n' })
  f.write('copied.md', 'copy contents\n'); f.git('add', '--all')
  const raw = f.diff('--cached', '--find-copies', '--find-copies-harder')
  const views = reviewFileDiffs(raw, manifest('original.md', 'copied.md'))
  assert.match(views.get('copied.md'), /copy to copied.md/)
  assert.equal(views.has('original.md'), false)
})

test('real mode-only, binary patch and empty-file addition need no text hunks', t => {
  const f = fixture(t, { 'script.sh': 'echo hello\n', 'binary.bin': Buffer.from([0, 1, 2, 3]) })
  chmodSync(join(f.dir, 'script.sh'), 0o755)
  f.write('binary.bin', Buffer.from([0, 4, 5, 6])); f.write('empty.md', '')
  f.git('add', '--all')
  const views = reviewFileDiffs(f.diff('--cached'), manifest('script.sh', 'binary.bin', 'empty.md'))
  assert.match(views.get('script.sh'), /old mode 100644\nnew mode 100755/)
  assert.match(views.get('binary.bin'), /GIT binary patch/)
  assert.match(views.get('empty.md'), /new file mode/)
})

test('backend JSON-quoted untracked blocks retain control/Unicode paths and b/ prefixes', () => {
  const paths = ['new space.md', 'tab\t.md', 'quote"\\.md', 'control\u0001.md', '🦀/план.md', 'b/nested b/file.md']
  const raw = paths.map(path => synthetic(path)).join('\n')
  const views = reviewFileDiffs(raw, paths.map(path => ({ path, status: '??' })))
  assert.equal(views.size, paths.length)
  for (const path of paths) assert.match(views.get(path), /\+new content/)
})

test('untracked block appended after staged changes is never labeled staged in its file view', t => {
  const f = fixture(t, { 'staged.md': 'before\n' })
  f.write('staged.md', 'after\n'); f.git('add', '--', 'staged.md')
  const raw = '# Staged changes\n\n' + f.diff('--cached') + '\n' + synthetic('new.md')
  const views = reviewFileDiffs(raw, [{ path: 'staged.md', status: 'M ' }, { path: 'new.md', status: '??' }])
  assert.match(views.get('staged.md'), /^# Staged changes/)
  assert.match(views.get('new.md'), /^# Working tree changes/)
  assert.doesNotMatch(views.get('new.md'), /# Staged changes/)
})

test('synthetic empty file and binary marker are mapped to verified headers', () => {
  const empty = synthetic('empty.md', '', 0)
  const quoted = JSON.stringify('b/binary.bin')
  const binary = `diff --git ${quoted} ${quoted}\nnew file mode 100644\n--- /dev/null\n+++ ${quoted}\nBinary file (content included in snapshot)\n`
  const views = reviewFileDiffs(empty + '\n' + binary, manifest('empty.md', 'binary.bin'))
  assert.match(views.get('empty.md'), /@@ -0,0 \+1,0 @@/)
  assert.match(views.get('binary.bin'), /Binary file/)
})

test('hunk text resembling diff headers, section headings and file metadata never splits files', t => {
  const content = 'diff --git a/other.md b/other.md\n# Staged changes\n+++ b/other.md\nrename to other.md\n'
  const f = fixture(t, { 'notes.md': content, 'other.md': 'other\n' })
  f.write('notes.md', content + 'appended\n')
  const views = reviewFileDiffs(f.diff(), manifest('notes.md', 'other.md'))
  assert.equal(views.size, 1)
  assert.match(views.get('notes.md'), /\+appended/)
  assert.match(views.get('notes.md'), / # Staged changes/)
})

test('unknown paths, conflicting metadata, invalid quoting and partial hunks stay unmapped', () => {
  const variants = [
    synthetic('unknown.md'),
    'diff --git a/notes.md b/notes.md\n--- a/other.md\n+++ b/notes.md\n@@ -1 +1 @@\n-old\n+new\n',
    'diff --git "a/notes.md b/notes.md\nold mode 100644\nnew mode 100755\n',
    'diff --git a/notes.md b/notes.md\n--- a/notes.md\n+++ b/notes.md\n@@ -1 +1,2 @@\n-old\n+new\n',
    'diff --git a/notes.md b/notes.md\nold mode 100644\n',
    'diff --git a/notes.md b/notes.md\n',
    synthetic('notes.md').trimEnd(),
    'diff --git "a/\\377.md" "b/\\377.md"\nold mode 100644\nnew mode 100755\n',
  ]
  for (const raw of variants) assert.equal(reviewFileDiffs(raw, manifest('notes.md', 'other.md', '�.md')).size, 0, raw)
})

test('incomplete binary patch stays unmapped while unrelated complete blocks survive', () => {
  const broken = 'diff --git a/binary.bin b/binary.bin\nindex aaaa..bbbb 100644\nGIT binary patch\nliteral 4\n'
  const views = reviewFileDiffs(synthetic('notes.md') + '\n' + broken, manifest('notes.md', 'binary.bin'))
  assert.equal(views.size, 1)
  assert.equal(views.has('binary.bin'), false)
  const cutAfterForwardParagraph = 'diff --git a/binary.bin b/binary.bin\nindex aaaa..bbbb 100644\nGIT binary patch\nliteral 4\nKcmZQzU|?Vb00001\n\n'
  assert.equal(reviewFileDiffs(cutAfterForwardParagraph, manifest('binary.bin')).size, 0)
})

test('ambiguous unquoted rename header without supporting metadata is not guessed', () => {
  const raw = 'diff --git a/from b/first b/second\nold mode 100644\nnew mode 100755\n'
  assert.equal(reviewFileDiffs(raw, manifest('first b/second', 'second')).size, 0)
})
