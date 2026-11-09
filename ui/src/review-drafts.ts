/** Short-lived, memory-only drafts survive tab/HEAD refreshes. Keys include the
 * authenticated project/root/file context; no code or credentials enter storage. */
const drafts = new Map<string, { editor: unknown; text: string; expires: number }>()
const TTL = 60 * 60 * 1000
export function readReviewDraft<T>(key: string): { editor: T | null; text: string } {
  const draft = drafts.get(key)
  if (!draft || draft.expires < Date.now()) { drafts.delete(key); return { editor: null, text: '' } }
  return { editor: draft.editor as T | null, text: draft.text }
}
export function keepReviewDraft(key: string, editor: unknown, text: string): void {
  if (!editor && !text) { drafts.delete(key); return }
  drafts.delete(key); drafts.set(key, { editor, text, expires: Date.now() + TTL })
  while (drafts.size > 40) drafts.delete(drafts.keys().next().value!)
}
