export interface TerminalSize { cols: number; rows: number }
export interface TerminalResize extends TerminalSize { offset: number }
interface Output { data_base64: string; geometry?: TerminalSize; resizes?: TerminalResize[] }
interface Sink {
  cols: number; rows: number;
  resize(cols: number, rows: number): void;
  write(bytes: Uint8Array, done: () => void, replay: boolean): void;
}
function validSize(value: unknown): value is TerminalSize {
  const size = value as TerminalSize | undefined
  return !!size && Number.isInteger(size.cols) && size.cols >= 20 && size.cols <= 300
    && Number.isInteger(size.rows) && size.rows >= 5 && size.rows <= 150
}

/** Replay bytes at their original PTY dimensions. A narrow viewer must never
 * reinterpret absolute cursor positions at its own width, or resize the agent.
 * Validate the complete frame before applying any part of it. */
export async function applyTerminalOutput(output: Output, sink: Sink, followGeometry: boolean, liveBytes = Infinity): Promise<boolean> {
  const bytes = Uint8Array.from(atob(output.data_base64), c => c.charCodeAt(0))
  if (followGeometry && output.geometry === undefined) return false
  const resizes = followGeometry ? output.resizes ?? [] : []
  if (followGeometry) {
    if (!validSize(output.geometry) || !Array.isArray(resizes) || resizes.length > 512) throw new Error('Некорректные размеры терминала.')
    let previous = 0
    for (const mark of resizes) {
      if (!validSize(mark) || !Number.isInteger(mark.offset) || mark.offset <= previous || mark.offset > bytes.length) throw new Error('Некорректные размеры терминала.')
      previous = mark.offset
    }
    if (sink.cols !== output.geometry.cols || sink.rows !== output.geometry.rows) sink.resize(output.geometry.cols, output.geometry.rows)
  }
  const replayEnd = Math.max(0, bytes.length - liveBytes)
  const write = async (start: number, end: number) => {
    if (start < replayEnd && start < end) {
      const boundary = Math.min(end, replayEnd)
      await new Promise<void>(resolve => sink.write(bytes.subarray(start, boundary), resolve, true))
      start = boundary
    }
    if (end > start) await new Promise<void>(resolve => sink.write(bytes.subarray(start, end), resolve, false))
  }
  let offset = 0
  for (const mark of resizes) {
    await write(offset, mark.offset)
    sink.resize(mark.cols, mark.rows)
    offset = mark.offset
  }
  await write(offset, bytes.length)
  return true
}
