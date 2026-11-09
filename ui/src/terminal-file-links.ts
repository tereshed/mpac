import { t } from './i18n.ts'
/** Local references only. No filesystem probing, shell expansion or URL opening. */
export interface TerminalFileLink { start: number; end: number; text: string }
export interface TerminalFileRoot { id: string; path: string }
export interface TerminalFileLocation { rootId: string; path: string; line?: number; column?: number; endLine?: number }
export const MAX_TERMINAL_LINK_ROWS = 16
const MAX_TEXT = 8192
const MAX_REFERENCE = 2048
const QUOTES = new Set(['"', "'", String.fromCharCode(96)])
const unsafe = (value: string) => /[\u0000-\u001f\u007f-\u009f\u2028-\u202e\u2066-\u2069]/u.test(value)

function unquote(value: string): string {
  if (value.length > 1 && ((QUOTES.has(value[0]) && value.at(-1) === value[0]) || (value[0] === '<' && value.at(-1) === '>') || (value[0] === '(' && value.at(-1) === ')'))) return value.slice(1, -1)
  return value
}
function positive(value: string): number {
  const number = Number(value)
  if (!Number.isSafeInteger(number) || number < 1 || number > 10_000_000) throw new Error(t("В ссылке указан недопустимый номер строки или столбца."))
  return number
}
function parseReference(reference: string): { path: string; line?: number; column?: number; endLine?: number } {
  if (!reference || reference.length > MAX_REFERENCE || unsafe(reference)) throw new Error(t("Ссылка на файл содержит недопустимые символы или слишком длинная."))
  let value = unquote(reference.trim())
  let line: number | undefined, column: number | undefined, endLine: number | undefined
  const hash = /#L(\d+)(?:-L?(\d+))?$/.exec(value)
  if (hash) {
    line = positive(hash[1]); endLine = hash[2] ? positive(hash[2]) : undefined; value = value.slice(0, hash.index)
    if (endLine !== undefined && endLine < line) throw new Error(t("Конец диапазона должен находиться после начала."))
  } else {
    const colon = /:(\d+)(?::(\d+))?$/.exec(value)
    if (colon) { line = positive(colon[1]); column = colon[2] ? positive(colon[2]) : undefined; value = value.slice(0, colon.index) }
  }
  value = unquote(value)
  if (/^file:/i.test(value)) {
    let url: URL
    try { url = new URL(value) } catch { throw new Error(t("Некорректная локальная ссылка file://.")) }
    if (url.protocol !== 'file:' || !['', 'localhost'].includes(url.hostname.toLowerCase()) || url.username || url.password || url.port || url.search || url.hash) throw new Error(t("Открываются только локальные file:// ссылки без удалённого адреса."))
    try { value = decodeURIComponent(url.pathname) } catch { throw new Error(t("Не удалось прочитать путь в ссылке file://.")) }
    if (/^\/[a-z]:[\/\\]/i.test(value)) value = value.slice(1)
  } else if (/^[a-z][a-z\d+.-]*:/i.test(value) && !/^[a-z]:[\/\\]/i.test(value)) throw new Error(t("Можно открывать только локальные файлы проекта."))
  if (!value || unsafe(value)) throw new Error(t("Путь файла пуст или содержит недопустимые символы."))
  if (/^~(?:[\/\\]|$)/.test(value)) throw new Error(t("Для пути с ~ укажите полный путь файла или путь относительно рабочей папки сессии."))
  if (/^(?:\\\\|\/\/)/.test(value)) throw new Error(t("Сетевые пути не открываются через терминальные ссылки."))
  return { path: value, ...(line === undefined ? {} : { line }), ...(column === undefined ? {} : { column }), ...(endLine === undefined ? {} : { endLine }) }
}
function windows(path: string): boolean { return /^[a-z]:[\/\\]/i.test(path) }
function windowsFold(value: string): string { return value.replace(/[A-Z]/g, letter => letter.toLowerCase()) }
function normalized(path: string, win: boolean): string {
  let value = win ? path.replace(/\\/g, '/') : path
  const prefix = win ? value.slice(0, 2) + '/' : '/'
  if (win ? !/^[a-z]:\//i.test(value) : !value.startsWith('/')) throw new Error(t("Рабочая папка проекта должна иметь абсолютный путь."))
  value = win ? value.slice(3) : value.slice(1)
  const segments: string[] = []
  for (const part of value.split('/')) {
    if (!part || part === '.') continue
    if (part === '..') { if (!segments.length) throw new Error(t("Путь выходит за пределы доступных папок проекта.")); segments.pop() }
    else segments.push(part)
  }
  return prefix + segments.join('/')
}
export function resolveTerminalFileLink(reference: string, mainRoot: TerminalFileRoot, roots: TerminalFileRoot[]): TerminalFileLocation {
  const parsed = parseReference(reference)
  const win = windows(mainRoot.path)
  const cwd = normalized(mainRoot.path, win)
  const absoluteWin = windows(parsed.path)
  const absolute = parsed.path.startsWith('/') || absoluteWin
  const targetWin = absolute ? absoluteWin : win
  const target = normalized(absolute ? parsed.path : cwd + (cwd.endsWith('/') ? '' : '/') + parsed.path, targetWin)
  const folded = targetWin ? windowsFold(target) : target
  const permitted = roots.flatMap(root => {
    if (windows(root.path) !== targetWin) return []
    let path: string
    try { path = normalized(root.path, targetWin) } catch { return [] }
    const compare = targetWin ? windowsFold(path) : path
    const prefix = compare.endsWith('/') ? compare : compare + '/'
    return folded === compare || folded.startsWith(prefix) ? [{ root, path }] : []
  }).sort((a, b) => b.path.length - a.path.length)
  const chosen = permitted[0]
  if (!chosen) throw new Error(t("Файл находится вне папок этого проекта. Добавьте нужную папку в проект, чтобы открыть его."))
  const path = target.slice(chosen.path.length).replace(/^\//, '')
  if (!path) throw new Error(t("Ссылка указывает на папку. Выберите файл внутри неё."))
  return { rootId: chosen.root.id, path, ...(parsed.line === undefined ? {} : { line: parsed.line }), ...(parsed.column === undefined ? {} : { column: parsed.column }), ...(parsed.endLine === undefined ? {} : { endLine: parsed.endLine }) }
}
function looksLocal(reference: string): boolean {
  try {
    const parsed = parseReference(reference), path = parsed.path
    if (/[\/\\]$/.test(path)) return false
    return /^file:/i.test(unquote(reference.trim())) || windows(path) || path.startsWith('/') || path.includes('/') || path.includes('\\') || /(?:^|\/)[^/]+\.[\p{L}\p{N}_-]{1,16}$/u.test(path) || /^(?:README|LICENSE|CHANGELOG|Dockerfile|Makefile|Gemfile|Procfile)$/i.test(path)
  } catch { return false }
}
export function findTerminalFileLinks(text: string): TerminalFileLink[] {
  if (!text || text.length > MAX_TEXT) return []
  const found: TerminalFileLink[] = []
  const add = (start: number, end: number) => {
    if (found.length >= 256 || start >= end || end - start > MAX_REFERENCE || found.some(link => start < link.end && end > link.start)) return
    const reference = text.slice(start, end)
    if (looksLocal(reference)) found.push({ start, end, text: reference })
  }
  // Markdown targets preserve spaces. Balanced parentheses belong to the path.
  let markdownBudget = MAX_TEXT * 2
  for (let i = 0; i < text.length - 1 && markdownBudget > 0; i++) {
    if (text[i] !== ']' || text[i + 1] !== '(') continue
    let depth = 1, end = i + 2
    for (; end < text.length && end - i <= MAX_REFERENCE && markdownBudget-- > 0; end++) {
      if (text[end] === '(') depth++
      if (text[end] === ')' && --depth === 0) break
    }
    if (depth === 0) add(i + 2, end)
  }
  // Quoted paths can include spaces and may have :line:column after the quote.
  for (let start = 0; start < text.length; start++) {
    const quote = text[start]
    if (!QUOTES.has(quote) && quote !== '<') continue
    const end = text.indexOf(quote === '<' ? '>' : quote, start + 1)
    if (end < 0 || end - start > MAX_REFERENCE) continue
    const suffix = /^(?::\d+(?::\d+)?|#L\d+(?:-L?\d+)?)/.exec(text.slice(end + 1))
    add(start, end + 1 + (suffix?.[0].length || 0)); start = end
  }
  // UTF-16 match offsets are used directly; xterm cell mapping happens below.
  const tokens = /[^\s"'\x60<>()[\]{};,]+/gu
  for (const match of text.matchAll(tokens)) {
    const start = match.index!
    let value = match[0]
    value = value.replace(/[.!?:]+$/, '')
    add(start, start + value.length)
  }
  return found.sort((a, b) => a.start - b.start)
}

export interface TerminalLinkCell { getChars(): string; getWidth(): number }
export interface TerminalLinkRow { isWrapped: boolean; length: number; getCell(index: number): TerminalLinkCell | undefined }
export interface TerminalCellLink { text: string; range: { start: { x: number; y: number }; end: { x: number; y: number } } }
/** One complete soft-wrapped logical line. Coordinates are public xterm 1-based
 * buffer cells, including CJK/emoji width and combining-character clusters. */
export function findTerminalFileLinksInRows(rows: TerminalLinkRow[], firstBufferLine: number): TerminalCellLink[] {
  if (!rows.length || rows.length > MAX_TERMINAL_LINK_ROWS || rows[0].isWrapped || rows.slice(1).some(row => !row.isWrapped)) return []
  let text = ''
  const cells: { start: { x: number; y: number }; end: { x: number; y: number } }[] = []
  for (let y = 0; y < rows.length; y++) {
    const row = rows[y], width = Math.min(row.length, 300)
    let last = width - 1
    // Null padding before a wrapped wide character is not an actual space.
    while (last >= 0 && !(row.getCell(last)?.getChars())) last--
    for (let x = 0; x <= last; x++) {
      const cell = row.getCell(x)
      if (!cell || cell.getWidth() === 0) continue
      const chars = cell.getChars() || ' '
      text += chars
      for (let offset = 0; offset < chars.length; offset++) cells.push({ start: { x: x + 1, y: firstBufferLine + y }, end: { x: Math.min(width, x + Math.max(1, cell.getWidth())), y: firstBufferLine + y } })
      if (text.length > MAX_TEXT) return []
    }
  }
  return findTerminalFileLinks(text).flatMap(link => cells[link.start] && cells[link.end - 1] ? [{ text: link.text, range: { start: cells[link.start].start, end: cells[link.end - 1].end } }] : [])
}
