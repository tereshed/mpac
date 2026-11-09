import { setLocale } from '../src/i18n.ts'
setLocale('ru', false)
import assert from 'node:assert/strict'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import {
  attachmentBase64, clipboardFiles, composeTerminalDraft, MAX_ATTACHMENT_BYTES,
  MAX_STAGED_BYTES, MAX_STAGED_FILES, MAX_TERMINAL_INPUT_BYTES, previewableImage,
  quoteAttachmentPath, terminalInputBytes, validateAttachment,
  validateComposerBeforeUpload, validateTerminalPaste,
} from '../src/terminal-input.ts'

test('multiline paste requires the CLI bracketed-paste mode; plain text remains accepted', () => {
  assert.doesNotThrow(() => validateTerminalPaste('A normal task', false))
  for (const text of ['one\ntwo', 'one\r\ntwo', 'one\rtwo', 'one\u2028two']) {
    assert.throws(() => validateTerminalPaste(text, false), /многострочной вставки/)
    assert.doesNotThrow(() => validateTerminalPaste(text, true))
  }
})

test('pasted ESC bracket closers, C0, DEL and C1 control sequences are rejected', () => {
  for (const text of ['ok\x1b[201~\rdo-something', 'ok\x00bad', 'ok\x03bad', 'ok\x7fbad', 'ok\u009b201~bad', 'ok\u0085bad']) {
    assert.throws(() => validateTerminalPaste(text, true), /управляющие символы/)
  }
  assert.doesNotThrow(() => validateTerminalPaste('normal\tindented\nnext', true))
})

test('UTF-8 byte budget includes bracketed-paste wrappers and xterm newline conversion', () => {
  assert.equal(terminalInputBytes('a\r\nb\nc', true), 17)
  assert.equal(terminalInputBytes('я', true), 14)
  assert.doesNotThrow(() => validateTerminalPaste('x'.repeat(MAX_TERMINAL_INPUT_BYTES - 12), true))
  assert.throws(() => validateTerminalPaste('x'.repeat(MAX_TERMINAL_INPUT_BYTES - 11), true), /16 KiB/)
  assert.throws(() => validateTerminalPaste('я'.repeat(MAX_TERMINAL_INPUT_BYTES / 2), true), /16 KiB/)
})

test('impossible draft size and unsupported multiline are rejected before uploading files', () => {
  assert.throws(() => validateComposerBeforeUpload('x'.repeat(MAX_TERMINAL_INPUT_BYTES - 12), [], 1, true), /16 KiB/)
  assert.throws(() => validateComposerBeforeUpload('one\ntwo', [], 1, false), /многострочной вставки/)
  assert.doesNotThrow(() => validateComposerBeforeUpload('task', ['/tmp/already uploaded.png'], 1, true))
})

test('quoted attachment paths stay literal in a real POSIX shell', () => {
  const path = '/tmp/spaces \'";$(printf injected) $HOME `printf nope` \\ файл.png'
  const quote = quoteAttachmentPath(path)
  const result = spawnSync('/bin/sh', ['-c', `printf '%s' ${quote}`], { encoding: 'utf8' })
  assert.equal(result.status, 0)
  assert.equal(result.stdout, path)
})

test('unsafe attachment paths cannot inject terminal controls or line breaks', () => {
  for (const path of ['/tmp/a\nb', '/tmp/a\rb', '/tmp/a\tb', '/tmp/\x1b[201~', '/tmp/\u009b201~', '/tmp/\u2029', 'C:\\other\\file']) {
    assert.throws(() => quoteAttachmentPath(path), /неподдерживаемый путь/)
  }
})

test('composer inserts literal tokens and never adds an Enter or newline itself', () => {
  assert.equal(composeTerminalDraft('Review this', ['/tmp/a b.png']), "Review this '/tmp/a b.png'")
  assert.equal(composeTerminalDraft('', ['/tmp/a', '/tmp/b']), "'/tmp/a' '/tmp/b'")
  assert.equal(composeTerminalDraft('one\ntwo', []), 'one\ntwo')
  assert.equal(composeTerminalDraft('one\ntwo\n', ['/tmp/a']), "one\ntwo\n'/tmp/a'")
})

test('staging enforces file, count, combined-size and filename constraints', () => {
  assert.doesNotThrow(() => validateAttachment({ name: "a b'c.png", size: MAX_ATTACHMENT_BYTES }, 0, 0))
  assert.throws(() => validateAttachment({ name: 'too-big.png', size: MAX_ATTACHMENT_BYTES + 1 }, 0, 0), /10 MiB/)
  assert.throws(() => validateAttachment({ name: 'one.png', size: 1 }, MAX_STAGED_FILES, 0), /10 файлов/)
  assert.throws(() => validateAttachment({ name: 'one.png', size: 1 }, 1, MAX_STAGED_BYTES), /50 MiB/)
  for (const name of ['../outside.png', 'a/b.txt', 'a\\b.txt', 'a\nb.txt', '\x1b[201~.png', '..']) {
    assert.throws(() => validateAttachment({ name, size: 1 }, 0, 0), /Имя файла/)
  }
})

test('previews allow local raster images but never SVG or HTML', () => {
  for (const type of ['image/png', 'image/jpeg', 'image/webp', 'image/gif']) assert.equal(previewableImage(type), true)
  for (const type of ['image/svg+xml', 'text/html', 'application/pdf', '']) assert.equal(previewableImage(type), false)
})

test('clipboard file extraction prefers real files and falls back to file items only', () => {
  const first = { name: 'first.png' }, second = { name: 'second.png' }
  assert.deepEqual(clipboardFiles({ files: [first], items: [{ kind: 'file', getAsFile: () => second }] }), [first])
  assert.deepEqual(clipboardFiles({ files: [], items: [{ kind: 'string' }, { kind: 'file', getAsFile: () => second }, { kind: 'file', getAsFile: () => null }] }), [second])
  assert.deepEqual(clipboardFiles(null), [])
})

test('base64 reads original bytes and abort prevents a late upload after file read', async () => {
  const bytes = Uint8Array.from([0, 255, 127, 42, 128])
  const controller = new AbortController()
  assert.equal(await attachmentBase64({ arrayBuffer: async () => bytes.buffer }, controller.signal), Buffer.from(bytes).toString('base64'))
  controller.abort()
  let read = false
  await assert.rejects(attachmentBase64({ arrayBuffer: async () => { read = true; return bytes.buffer } }, controller.signal), { name: 'AbortError' })
  assert.equal(read, false)
  const late = new AbortController()
  await assert.rejects(attachmentBase64({ arrayBuffer: async () => { late.abort(); return bytes.buffer } }, late.signal), { name: 'AbortError' })
})
