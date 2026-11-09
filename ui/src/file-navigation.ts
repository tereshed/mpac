export interface FileOpenRequest {
  requestId: string;
  epoch: number;
  projectId: string;
  rootId: string;
  path: string;
  line?: number;
  column?: number;
  endLine?: number;
}

/** Bounded display around a requested source line, with real source numbers. */
export function sourceLineWindow(total: number, line?: number, limit = 2000): { start: number; end: number; found: boolean } {
  const count = Math.max(0, Math.floor(total))
  const size = Math.max(1, Math.min(2000, Math.floor(limit)))
  const found = line === undefined || (Number.isSafeInteger(line) && line > 0 && line <= count)
  const start = found && line !== undefined ? Math.max(0, Math.min(line - 1 - 100, count - size)) : 0
  return { start, end: Math.min(count, start + size), found }
}
