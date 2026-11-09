export interface TerminalKey {
  type: string; key: string; shiftKey: boolean; ctrlKey: boolean;
  altKey: boolean; metaKey: boolean; isComposing: boolean; keyCode?: number;
}

/** Extended Enter retains its modifier. Never substitute bare CR or LF: some
 * CLIs (including Hermes on POSIX) bind both of those to submit. */
export const SHIFT_ENTER_SEQUENCE = '\x1b[13;2u'

export function isTerminalNewlineKey(event: TerminalKey): boolean {
  return event.key === 'Enter' && event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing && event.keyCode !== 229
}

export function directNewlineSequence(agent: string): string | null {
  return ['claude', 'zai', 'codex', 'hermes'].includes(agent) ? SHIFT_ENTER_SEQUENCE : null
}
