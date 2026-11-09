/** Explicit touch keys use the same VT sequences as a physical keyboard.
 * Moving a selection never sends Enter or guesses a numbered choice. */
export type TerminalControl = 'submit' | 'newline' | 'escape' | 'up' | 'down' | 'left' | 'right' | 'tab' | 'space' | 'backspace'
export function terminalControlSequence(action: Exclude<TerminalControl, 'newline'>, applicationCursor = false): string {
  const arrows = { up: 'A', down: 'B', right: 'C', left: 'D' } as const
  if (action in arrows) return `\x1b${applicationCursor ? 'O' : '['}${arrows[action as keyof typeof arrows]}`
  return { submit: '\r', escape: '\x1b', tab: '\t', space: ' ', backspace: '\x7f' }[action as 'submit' | 'escape' | 'tab' | 'space' | 'backspace']
}
