import { t } from './i18n.ts'
export const MAX_TERMINAL_INPUT_BYTES = 16 * 1024
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024
export const MAX_STAGED_FILES = 10
export const MAX_STAGED_BYTES = 50 * 1024 * 1024

const unsafePaste = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/
const unsafePath = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/

export function terminalInputBytes(text: string, bracketedPaste: boolean): number {
  // xterm normalizes LF/CRLF to CR before wrapping bracketed paste.
  return new TextEncoder().encode(text.replace(/\r?\n/g, '\r')).byteLength + (bracketedPaste ? 12 : 0)
}

export function validateTerminalPaste(text: string, bracketedPaste: boolean, reservedBytes = 0): void {
  if (unsafePaste.test(text)) throw new Error(t("Черновик содержит управляющие символы. Удалите их перед вставкой."))
  if (/[\r\n\u2028\u2029]/.test(text) && !bracketedPaste) {
    throw new Error(t("Этот CLI сейчас не сообщает поддержку безопасной многострочной вставки. Черновик сохранён: дождитесь поля ввода CLI или вставьте одну строку. Enter автоматически не отправляется."))
  }
  if (terminalInputBytes(text, bracketedPaste) + reservedBytes > MAX_TERMINAL_INPUT_BYTES) {
    throw new Error(t("Черновик вместе с путями файлов превышает 16 KiB. Сократите текст или число файлов перед вставкой."))
  }
}

export function quoteAttachmentPath(path: string): string {
  if (!path.startsWith('/') || unsafePath.test(path)) throw new Error(t("Сервер вернул неподдерживаемый путь вложения. Файл не вставлен в терминал."))
  // Interactive PTY currently runs on macOS/Linux. POSIX single quotes keep
  // filenames literal even if the user later sends this draft to a shell.
  return "'" + path.replace(/'/g, "'\\''") + "'"
}

export function composeTerminalDraft(text: string, paths: string[]): string {
  const quoted = paths.map(quoteAttachmentPath).join(' ')
  return text + (quoted ? (text && !/\s$/.test(text) ? ' ' : '') + quoted : '')
}

export function validateComposerBeforeUpload(text: string, existingPaths: string[], pendingFiles: number, bracketedPaste: boolean): void {
  // Unknown upload paths are checked again after upload. Reserve the minimum
  // possible quoted absolute-path token before making any upload request.
  validateTerminalPaste(composeTerminalDraft(text, existingPaths), bracketedPaste, pendingFiles * 4)
}

export function validateAttachment(file: { name: string; size: number }, existingCount: number, existingBytes: number): void {
  if (!file.name || file.name === '.' || file.name === '..' || /[\\/]/.test(file.name) || unsafePath.test(file.name)) {
    throw new Error(t("Имя файла содержит неподдерживаемые символы. Переименуйте файл перед прикреплением."))
  }
  if (file.size > MAX_ATTACHMENT_BYTES) throw new Error(t("Файл «{p0}» больше 10 MiB.", { p0: file.name }))
  if (existingCount >= MAX_STAGED_FILES) throw new Error(t("В один черновик можно добавить не более 10 файлов."))
  if (existingBytes + file.size > MAX_STAGED_BYTES) throw new Error(t("Общий размер файлов черновика превышает 50 MiB."))
}

export function previewableImage(type: string): boolean {
  return ['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(type.toLowerCase())
}

export function clipboardFiles(transfer: DataTransfer | null): File[] {
  if (!transfer) return []
  if (transfer.files.length) return [...transfer.files]
  return [...transfer.items].filter(item => item.kind === 'file').map(item => item.getAsFile()).filter((file): file is File => file !== null)
}

export async function attachmentBase64(file: Pick<File, 'arrayBuffer'>, signal: AbortSignal): Promise<string> {
  if (signal.aborted) throw new DOMException('Aborted', 'AbortError')
  const bytes = new Uint8Array(await file.arrayBuffer())
  if (signal.aborted) throw new DOMException('Aborted', 'AbortError')
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 8192) binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192))
  return btoa(binary)
}
