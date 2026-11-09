/** Touch scrolling is local viewing only: never translate a swipe to CLI keys. */
export class TerminalTouchScroll {
  private origin: { x: number; y: number } | null = null
  private lastY = 0
  private remainder = 0
  private dragging = false
  start(x: number, y: number) { this.origin = { x, y }; this.lastY = y; this.remainder = 0; this.dragging = false }
  end() { this.origin = null; this.dragging = false; this.remainder = 0 }
  move(x: number, y: number, lineHeight: number): { scrolling: boolean; lines: number } {
    if (!this.origin || lineHeight <= 0 || !Number.isFinite(lineHeight)) return { scrolling: false, lines: 0 }
    if (!this.dragging) {
      const dx = Math.abs(x - this.origin.x), dy = Math.abs(y - this.origin.y)
      if (dy < 6 || dy <= dx) return { scrolling: false, lines: 0 }
      this.dragging = true
    }
    const distance = this.remainder + this.lastY - y
    const lines = Math.trunc(distance / lineHeight)
    this.remainder = distance - lines * lineHeight; this.lastY = y
    return { scrolling: true, lines }
  }
}

interface HistoryBuffer {
  length: number
  getLine(index: number): { isWrapped: boolean; translateToString(trimRight?: boolean): string } | undefined
}
/** A stable, read-only snapshot of parsed text, not raw escape sequences.
 * No claim of a full transcript: alternate-screen CLIs may retain less history. */
export function terminalHistory(buffer: HistoryBuffer, maxLines = 5000, maxChars = 1024 * 1024): string {
  const rows: string[] = []
  let size = 0
  for (let index = buffer.length - 1; index >= Math.max(0, buffer.length - maxLines); index--) {
    const line = buffer.getLine(index)
    if (!line) continue
    const text = line.translateToString(true)
    if (rows.length && size + text.length > maxChars) break
    rows.push(text.slice(-maxChars)); size += text.length + 1
  }
  return rows.reverse().join('\n').trimEnd()
}

/** Join only terminal soft wraps; preserve real newlines and code indentation.
 * Reading happens after ANSI parsing at the original terminal geometry. */
export function terminalReadingText(buffer: HistoryBuffer, maxLines = 5000, maxChars = 1024 * 1024): string {
  const rows: { text: string; wrapped: boolean }[] = []
  let size = 0
  for (let index = buffer.length - 1; index >= Math.max(0, buffer.length - maxLines); index--) {
    const line = buffer.getLine(index)
    if (!line) continue
    let text = line.translateToString(!buffer.getLine(index + 1)?.isWrapped)
    if (rows.length && size + text.length + 1 > maxChars) break
    if (text.length > maxChars) {
      text = text.slice(0, maxChars)
      if (/[\uD800-\uDBFF]$/.test(text)) text = text.slice(0, -1)
    }
    rows.push({ text, wrapped: line.isWrapped }); size += text.length + 1
  }
  const logical: string[] = []
  for (const row of rows.reverse()) {
    if (row.wrapped && logical.length) logical[logical.length - 1] += row.text
    else logical.push(row.text)
  }
  return logical.join('\n').trimEnd()
}

/** Inline CLIs have real scrollback. Full-screen TUIs have a separate buffer
 * and must display the current interactive screen (not the previous prompt). */
export function terminalLiveText(buffer: HistoryBuffer & { baseY: number; type: string }, rows: number): string {
  if (buffer.type !== 'alternate') return terminalReadingText(buffer)
  return terminalReadingText({
    length: Math.min(rows, buffer.length - buffer.baseY),
    getLine: index => buffer.getLine(buffer.baseY + index),
  }).replace(/\n(?:[ \t]*\n){2,}/g, '\n\n')
}

/** Paste must be acknowledged before submit; uncertain paste never sends Enter. */
export async function insertAndMaybeSubmit(text: string, insert: (text: string) => Promise<void>, submit?: () => Promise<void>): Promise<void> {
  await insert(text)
  if (submit) await submit()
}
