/** Best-effort file views of our bounded Git review. Ambiguous or incomplete
 * blocks stay unmapped; callers must retain the authoritative All changes view. */

type ManifestFile = { path: string; status: string }
type Pair = { before: string; after: string; synthetic: boolean }

const decoder = new TextDecoder('utf-8', { fatal: true })
const encoder = new TextEncoder()

function unquote(value: string): string | null {
  if (!value.startsWith('"')) return /["\t\r\n]/.test(value) ? null : value
  try {
    const parsed: unknown = JSON.parse(value)
    if (typeof parsed === 'string' && decoder.decode(encoder.encode(parsed)) === parsed) return parsed
  } catch { /* Git C quoting also has octal UTF-8 and \a/\v escapes. */ }
  if (!value.endsWith('"')) return null
  const bytes: number[] = []
  const escapes: Record<string, number> = { a: 7, b: 8, t: 9, n: 10, v: 11, f: 12, r: 13, '"': 34, '\\': 92 }
  for (let i = 1; i < value.length - 1; i++) {
    const char = value[i]
    if (char === '"' || char.charCodeAt(0) < 32) return null
    if (char !== '\\') {
      const point = value.codePointAt(i)!
      bytes.push(...encoder.encode(String.fromCodePoint(point)))
      if (point > 0xffff) i++
      continue
    }
    const escape = value[++i]
    if (escape === undefined || i >= value.length - 1) return null
    if (/[0-7]/.test(escape)) {
      let octal = escape
      while (octal.length < 3 && i + 1 < value.length - 1 && /[0-7]/.test(value[i + 1])) octal += value[++i]
      const byte = Number.parseInt(octal, 8)
      if (byte > 255) return null
      bytes.push(byte)
    } else if (Object.hasOwn(escapes, escape)) bytes.push(escapes[escape])
    else return null
  }
  try { return decoder.decode(new Uint8Array(bytes)) } catch { return null }
}

function headerPairs(header: string): Pair[] {
  const text = header.slice('diff --git '.length)
  if (text.length > 16384) return []
  const result: Pair[] = []
  let splits = 0
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== ' ') continue
    if (++splits > 256) return []
    const before = unquote(text.slice(0, i))
    const after = unquote(text.slice(i + 1))
    if (!before || !after || !after.startsWith('b/')) continue
    if (before.startsWith('a/')) result.push({ before: before.slice(2), after: after.slice(2), synthetic: false })
    else if (before.startsWith('b/')) result.push({ before: before.slice(2), after: after.slice(2), synthetic: true })
  }
  return result
}

function fileHeader(value: string, prefix: 'a/' | 'b/'): string | null | undefined {
  // Git appends a tab to unquoted file headers containing spaces.
  const decoded = unquote(!value.startsWith('"') && value.endsWith('\t') ? value.slice(0, -1) : value)
  if (decoded === '/dev/null') return null
  return decoded?.startsWith(prefix) ? decoded.slice(2) : undefined
}

function blockFile(raw: string, known: Set<string>): string | null {
  // Backend Git and synthetic blocks end in LF; missing LF may be truncation.
  if (!raw.endsWith('\n')) return null
  const lines = raw.split('\n')
  const pairs = headerPairs(lines[0])
  if (!pairs.length) return null
  let before: string | null | undefined
  let after: string | null | undefined
  let from: string | undefined
  let to: string | undefined
  let oldMode = false, newMode = false, created = false, deleted = false, index = false
  let hunks = false, oldLeft = 0, newLeft = 0, inHunk = false
  let binary = false, binaryPatch = false, binaryParagraphs = 0, binaryData = 0
  for (const line of lines.slice(1)) {
    if (inHunk) {
      if (line.startsWith('\\ No newline at end of file')) continue
      if (line.startsWith(' ')) { oldLeft--; newLeft-- }
      else if (line.startsWith('-')) oldLeft--
      else if (line.startsWith('+')) newLeft--
      else return null
      if (oldLeft < 0 || newLeft < 0) return null
      inHunk = oldLeft > 0 || newLeft > 0
      continue
    }
    if (binaryPatch) {
      if (/^(literal|delta) \d+$/.test(line)) {
        if (binaryData) return null
        binaryData = -1
      } else if (!line) {
        if (binaryData > 0) { binaryParagraphs++; binaryData = 0 }
        else if (binaryData === -1) return null
      } else if (binaryData !== 0 && /^[A-Za-z][!-~]+$/.test(line)) binaryData = Math.max(0, binaryData) + 1
      else return null
      continue
    }
    const hunk = /^@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))? @@(?:.*)$/.exec(line)
    if (hunk) {
      if (before === undefined || after === undefined) return null
      oldLeft = hunk[1] === undefined ? 1 : Number(hunk[1])
      newLeft = hunk[2] === undefined ? 1 : Number(hunk[2])
      if (!Number.isSafeInteger(oldLeft) || !Number.isSafeInteger(newLeft)) return null
      hunks = true; inHunk = oldLeft > 0 || newLeft > 0
      continue
    }
    if (!line || (hunks && line.startsWith('\\ No newline at end of file'))) continue
    if (hunks) return null
    if (line.startsWith('--- ')) {
      if (before !== undefined) return null
      before = fileHeader(line.slice(4), 'a/')
      if (before === undefined) return null
    } else if (line.startsWith('+++ ')) {
      if (after !== undefined) return null
      after = fileHeader(line.slice(4), 'b/')
      if (after === undefined) return null
    } else if (line.startsWith('rename from ') || line.startsWith('copy from ')) {
      const value = unquote(line.slice(line.startsWith('rename') ? 12 : 10))
      if (value === null || from !== undefined) return null
      from = value
    } else if (line.startsWith('rename to ') || line.startsWith('copy to ')) {
      const value = unquote(line.slice(line.startsWith('rename') ? 10 : 8))
      if (value === null || to !== undefined) return null
      to = value
    } else if (/^old mode \d{6}$/.test(line)) oldMode = true
    else if (/^new mode \d{6}$/.test(line)) newMode = true
    else if (/^new file mode \d{6}$/.test(line)) created = true
    else if (/^deleted file mode \d{6}$/.test(line)) deleted = true
    else if (/^index [0-9a-f]+\.\.[0-9a-f]+(?: \d{6})?$/.test(line)) index = true
    else if (/^(dis)?similarity index \d+%$/.test(line)) { /* Rename/copy metadata. */ }
    else if (line === 'GIT binary patch') { binary = true; binaryPatch = true }
    else if (/^Binary files .+ differ$/.test(line) || line === 'Binary file (content included in snapshot)') binary = true
    else return null
  }
  if (inHunk || (binaryPatch && (binaryData !== 0 || binaryParagraphs !== 2))) return null
  if ((from === undefined) !== (to === undefined)) return null
  if (!hunks && !binary && !(oldMode && newMode) && !(from !== undefined && to !== undefined) && !((created || deleted) && index)) return null
  const targets = new Set<string>()
  for (const pair of pairs) {
    if (pair.synthetic && !(created && before === null)) continue
    if (before !== undefined && before !== null && pair.before !== before) continue
    if (after !== undefined && after !== null && pair.after !== after) continue
    if (from !== undefined && pair.before !== from) continue
    if (to !== undefined && pair.after !== to) continue
    if (pair.before !== pair.after && to === undefined && (before === undefined || after === undefined)) continue
    const target = after === null || deleted ? pair.before : pair.after
    if (known.has(target)) targets.add(target)
  }
  return targets.size === 1 ? [...targets][0] : null
}

export function reviewFileDiffs(diff: string, files: ManifestFile[]): Map<string, string> {
  const known = new Set(files.map(file => file.path))
  const statuses = new Map(files.map(file => [file.path, file.status]))
  const result = new Map<string, string>()
  let section = ''
  let block: string[] = []
  const finish = () => {
    if (!block.length) return
    const raw = block.join('')
    const path = blockFile(raw, known)
    if (path !== null) {
      // The backend appends untracked files after its last tracked section,
      // which can be staged when there were no tracked working-tree changes.
      const ownSection = statuses.get(path) === '??' ? '# Working tree changes' : section
      const view = (ownSection ? ownSection + '\n\n' : '') + raw
      result.set(path, (result.get(path) || '') + (result.has(path) ? '\n' : '') + view)
    }
    block = []
  }
  for (const line of diff.match(/[^\n]*\n|[^\n]+$/g) || []) {
    const text = line.endsWith('\n') ? line.slice(0, -1) : line
    if (text === '# Staged changes' || text === '# Working tree changes') {
      finish(); section = text
    } else if (text.startsWith('diff --git ')) {
      finish(); block.push(line)
    } else if (block.length) block.push(line)
  }
  finish()
  return result
}
