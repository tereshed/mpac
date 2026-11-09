import VoiceInput from './VoiceInput'
import type { VoiceSettings } from './voice'
import { localeTag, t } from './i18n.ts'
import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef, useState } from 'react'
import { api, errorMessage } from './api'
import { attachmentBase64, MAX_STAGED_FILES, previewableImage, validateAttachment, validateComposerBeforeUpload, composeTerminalDraft } from './terminal-input'
import './terminal-composer.css'
import { insertAndMaybeSubmit } from './terminal-mobile'

interface UploadedAttachment { id: string; name: string; path: string; size: number; mime_type?: string }
interface StagedAttachment { id: number; file: File; preview?: string; uploaded?: UploadedAttachment; error?: string }
export interface TerminalComposerHandle {
  stage: (text: string, files?: File[], notice?: string) => void
  focusIfOpen: () => boolean
  focusInput: () => void
  blurInput: () => void
}

function sizeLabel(bytes: number): string {
  return bytes < 1024 * 1024 ? `${Math.ceil(bytes / 1024).toLocaleString(localeTag())} KiB` : `${(bytes / 1024 / 1024).toLocaleString(localeTag(), { minimumFractionDigits: 1, maximumFractionDigits: 1 })} MiB`
}

const TerminalComposer = forwardRef<TerminalComposerHandle, {
  token: string; sessionId: string; voice?: VoiceSettings | null; running: boolean; visible: boolean; hidden?: boolean; mobile?: boolean; plainTerminal?: boolean; autoFocus?: boolean; attachmentsAvailable: boolean;
  bracketedPaste: () => boolean; onInsert: (text: string) => Promise<void>; onFocusTerminal: () => void;
  onSubmit?: () => Promise<void>;
}>(function TerminalComposer({ token, sessionId, voice, running, visible, hidden = false, mobile = false, plainTerminal = false, autoFocus = true, attachmentsAvailable, bracketedPaste, onInsert, onSubmit, onFocusTerminal }, forwardedRef) {
  const [open, setOpen] = useState(false)
  const [voiceActive, setVoiceActive] = useState(false)
  const [draft, setDraft] = useState('')
  const [files, setFiles] = useState<StagedAttachment[]>([])
  const [busy, setBusy] = useState(false)
  const [readingClipboard, setReadingClipboard] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [lastPaths, setLastPaths] = useState<string[]>([])
  const textarea = useRef<HTMLTextAreaElement>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const filesRef = useRef<StagedAttachment[]>([])
  const nextFileId = useRef(0)
  const pending = useRef(false)
  const clipboardPending = useRef(false)
  const mounted = useRef(true)
  const controller = useRef<AbortController | null>(null)
  const runningRef = useRef(running)
  const visibleRef = useRef(visible)
  runningRef.current = running
  visibleRef.current = visible

  const replaceFiles = (next: StagedAttachment[]) => { filesRef.current = next; setFiles(next) }
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false; controller.current?.abort()
      for (const item of filesRef.current) if (item.preview) URL.revokeObjectURL(item.preview)
    }
  }, [])
  useEffect(() => {
    if (!running) controller.current?.abort()
  }, [running])
  useEffect(() => {
    if (open && visible && autoFocus) {
      const frame = requestAnimationFrame(() => textarea.current?.focus())
      return () => cancelAnimationFrame(frame)
    }
  }, [open, visible, autoFocus])

  useLayoutEffect(() => {
    const field = textarea.current
    if (!field) return
    if (mobile && !open) { field.style.height = '0px'; field.style.height = `${Math.min(112, Math.max(44, field.scrollHeight))}px` }
    else field.style.removeProperty('height')
  }, [draft, mobile, open])

  const addFiles = (incoming: File[]) => {
    const staged = [...filesRef.current]
    const failures: string[] = []
    for (const file of incoming) {
      try {
        validateAttachment(file, staged.length, staged.reduce((sum, item) => sum + item.file.size, 0))
        let preview: string | undefined
        if (previewableImage(file.type)) { try { preview = URL.createObjectURL(file) } catch { /* Name/size remain available without previews. */ } }
        staged.push({ id: ++nextFileId.current, file, preview })
      } catch (failure) { failures.push(errorMessage(failure)) }
    }
    replaceFiles(staged)
    if (failures.length) setError(failures.slice(0, 3).join(' '))
  }
  const stage = (text: string, incoming: File[] = [], message = '') => {
    if (pending.current) { setError(t("Дождитесь завершения текущей вставки. Новый буфер не добавлен.")); return }
    setOpen(true); setError(''); setNotice(message)
    if (text) setDraft(previous => previous + (previous && !previous.endsWith('\n') ? '\n' : '') + text)
    if (incoming.length) addFiles(incoming)
  }
  useImperativeHandle(forwardedRef, () => ({ stage, blurInput: () => textarea.current?.blur(), focusInput: () => {
    const field = textarea.current
    if (!visible || hidden || !field) return
    // iPad may retain DOM focus after dismissing its software keyboard.
    // A new focus transition within this tap reliably requests it again.
    if (document.activeElement === field) field.blur()
    field.focus({ preventScroll: true })
  }, focusIfOpen: () => { if (!open || !visible) return false; textarea.current?.focus(); return true } }))

  const removeFile = (id: number) => {
    if (pending.current) return
    const item = filesRef.current.find(file => file.id === id)
    if (item?.preview) URL.revokeObjectURL(item.preview)
    replaceFiles(filesRef.current.filter(file => file.id !== id)); setError('')
  }
  const readClipboard = async () => {
    if (clipboardPending.current || pending.current) return
    clipboardPending.current = true; setReadingClipboard(true); setOpen(true); setError(''); setNotice('')
    try {
      const incoming: File[] = []
      const texts: string[] = []
      if (navigator.clipboard?.read) {
        const items = await navigator.clipboard.read()
        for (const [index, item] of items.entries()) {
          const image = item.types.find(type => type.startsWith('image/'))
          if (image) {
            const blob = await item.getType(image)
            const extension = ({ 'image/png': 'png', 'image/jpeg': 'jpg', 'image/webp': 'webp', 'image/gif': 'gif' } as Record<string, string>)[image] || 'img'
            incoming.push(new File([blob], `clipboard-${index + 1}.${extension}`, { type: image }))
          } else if (item.types.includes('text/plain')) texts.push(await (await item.getType('text/plain')).text())
        }
      } else if (navigator.clipboard?.readText) texts.push(await navigator.clipboard.readText())
      else throw new Error('clipboard_unavailable')
      if (!mounted.current) return
      if (!incoming.length && !texts.some(Boolean)) { setNotice(t("В буфере нет доступного текста или изображения. Можно вставить текст в поле вручную.")); return }
      stage(incoming.length ? '' : texts.join('\n'), incoming)
    } catch {
      if (mounted.current) setNotice(t("Браузер не дал прочитать буфер. Вставьте текст в поле через Ctrl/⌘+V или выберите файл кнопкой «Прикрепить»."))
    } finally {
      clipboardPending.current = false
      if (mounted.current) { setReadingClipboard(false); if (visibleRef.current) textarea.current?.focus() }
    }
  }

  const insert = async (submit = false) => {
    if (pending.current || voiceActive || !runningRef.current || (!draft && !filesRef.current.length)) return
    if (filesRef.current.length && !attachmentsAvailable) { setError(t("Прикрепление файлов недоступно в запущенном daemon. Черновик сохранён; можно убрать файлы и вставить только текст.")); return }
    const selected = [...filesRef.current]
    try {
      validateComposerBeforeUpload(draft, selected.flatMap(item => item.uploaded ? [item.uploaded.path] : []), selected.filter(item => !item.uploaded).length, bracketedPaste())
    } catch (failure) { setError(errorMessage(failure)); return }
    const abort = new AbortController()
    let inserted = false
    controller.current = abort; pending.current = true
    setBusy(true); setError(''); setNotice('')
    try {
      for (const item of selected) {
        if (item.uploaded) continue
        if (!runningRef.current || abort.signal.aborted || !mounted.current) throw new DOMException('Aborted', 'AbortError')
        try {
          const data_base64 = await attachmentBase64(item.file, abort.signal)
          const uploaded = await api<UploadedAttachment>(token, `/sessions/${sessionId}/attachments`, { name: item.file.name, data_base64 }, abort.signal)
          if (!mounted.current || abort.signal.aborted) throw new DOMException('Aborted', 'AbortError')
          item.uploaded = uploaded; item.error = undefined
          replaceFiles(filesRef.current.map(current => current.id === item.id ? { ...current, uploaded, error: undefined } : current))
        } catch (failure) {
          if (mounted.current && !abort.signal.aborted) replaceFiles(filesRef.current.map(current => current.id === item.id ? { ...current, error: errorMessage(failure) } : current))
          throw failure
        }
      }
      if (!mounted.current || abort.signal.aborted || !runningRef.current) throw new DOMException('Aborted', 'AbortError')
      const paths = selected.flatMap(item => item.uploaded ? [item.uploaded.path] : [])
      const text = composeTerminalDraft(draft, paths)
      // Recheck after upload: the CLI mode or resulting path size may change.
      validateComposerBeforeUpload(text, [], 0, bracketedPaste())
      await insertAndMaybeSubmit(text, async value => {
        await onInsert(value)
        inserted = true
        if (!mounted.current || abort.signal.aborted) throw new DOMException('Aborted', 'AbortError')
        setLastPaths(paths)
        for (const item of filesRef.current) if (item.preview) URL.revokeObjectURL(item.preview)
        replaceFiles([]); setDraft(''); setOpen(false)
      }, submit && onSubmit ? async () => {
        if (!mounted.current || abort.signal.aborted || !runningRef.current || !visibleRef.current) throw new DOMException('Aborted', 'AbortError')
        await onSubmit()
      } : undefined)
      if (!mounted.current || abort.signal.aborted) return
      setNotice(submit ? t('Задача отправлена в терминал.') : t("Черновик вставлен в терминал. Проверьте текст и нажмите Enter для отправки."))
      // Keep the real textarea editable and focused on touch devices; pending
      // edits are intercepted without toggling iOS readOnly. Async focus of
      // xterm's hidden input cannot reliably summon the iOS keyboard.
      if (!mobile) onFocusTerminal()
    } catch (failure) {
      if (mounted.current) setError(inserted ? t('Текст вставлен, но отправка не подтверждена. Проверьте терминал перед повтором.') : abort.signal.aborted ? t("Вставка остановлена. Черновик сохранён; проверьте состояние сессии.") : t("{p0} Черновик сохранён. Проверьте терминал перед повторной вставкой.", { p0: errorMessage(failure) }))
    } finally {
      if (controller.current === abort) controller.current = null
      pending.current = false
      if (mounted.current) setBusy(false)
    }
  }

  const voiceInput = voice?.enabled && visible && running && !hidden
    ? <VoiceInput token={token} settings={voice} disabled={busy} onActive={setVoiceActive} onError={setError}
        onText={text => stage(text, [], t('Текст добавлен в черновик. Проверьте его перед вставкой.'))} />
    : null

  return <div hidden={hidden} className={`terminal-composer ${mobile ? 'touch-composer' : ''} ${open ? 'expanded' : ''}`} >
    <div className="terminal-composer-toolbar" hidden={mobile && !open}>{!mobile && voiceInput}<button type="button" className={open ? 'active' : ''} aria-expanded={open} aria-controls="terminal-draft-panel" disabled={busy} onClick={() => { const next = !open; setOpen(next); if (next && visible) { if (mobile) textarea.current?.focus({ preventScroll: true }); else requestAnimationFrame(() => { if (visibleRef.current) textarea.current?.focus() }) } }}>{t("✎ Черновик")}{' '}{draft || files.length ? <span>●</span> : null}</button><button type="button" disabled={busy || readingClipboard} onClick={() => void readClipboard()}>{readingClipboard ? t("Читаем…") : t("Из буфера")}</button><button type="button" disabled={busy || !attachmentsAvailable || files.length >= MAX_STAGED_FILES} title={attachmentsAvailable ? t("Добавить изображения или документы") : t("Вложения недоступны в запущенном daemon")} onClick={() => { setOpen(true); fileInput.current?.click() }}>{t("＋ Прикрепить")}</button><span>{busy ? t("Подготавливаем вставку…") : t("Без автоматического Enter")}</span><input className="terminal-file-input" ref={fileInput} type="file" multiple onChange={event => { const chosen = [...(event.target.files || [])]; event.target.value = ''; stage('', chosen) }} /></div>
    {notice && <div className="terminal-compose-notice" role="status">{t(notice)}</div>}
    {error && <div className="terminal-compose-error" role="alert">{t(error)}</div>}
    <div id="terminal-draft-panel" className="terminal-draft-panel" hidden={!open && !mobile}>
      <label htmlFor="terminal-draft-text"><span>{t(plainTerminal ? "Ввод в терминал" : mobile ? "Сообщение агенту" : "Текст задачи")}</span><textarea id="terminal-draft-text" ref={textarea} value={draft} readOnly={!mobile && busy} aria-busy={busy} onBeforeInput={event => { if (pending.current) event.preventDefault() }} onChange={event => { if (pending.current) return; setDraft(event.target.value); setError('') }} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); void insert(mobile && !!onSubmit) } }} onPaste={event => { if (pending.current) { event.preventDefault(); return }; const incoming = [...event.clipboardData.files]; if (incoming.length) { event.preventDefault(); stage('', incoming) } }} rows={mobile && !open ? 1 : 4} placeholder={t(plainTerminal ? "Команда или текст…" : mobile && !open ? "Сообщение агенту…" : "Напишите задачу в несколько строк или вставьте текст из буфера…")} autoCapitalize="sentences" autoCorrect="off" spellCheck={false} enterKeyHint="enter" /></label>
      {files.length > 0 && <div className="terminal-staged-files">{files.map(item => <div key={item.id} className={`terminal-staged-file ${item.error ? 'failed' : ''}`}>
        {item.preview ? <img src={item.preview} alt={item.file.name} loading="lazy" decoding="async" /> : <span className="terminal-file-symbol">▱</span>}<div><strong>{item.file.name}</strong><small>{sizeLabel(item.file.size)} · {item.uploaded ? t("Скопирован в проект") : t("Не загружен")}</small>{item.uploaded && <code>{item.uploaded.path}</code>}{item.error && <p>{t(item.error)}</p>}</div><button type="button" disabled={busy} onClick={() => removeFile(item.id)} aria-label={t("Убрать из черновика {p0}", { p0: item.file.name })} title={t("Убрать из черновика")}>×</button>
      </div>)}</div>}
      {!attachmentsAvailable && files.length > 0 && <p className="terminal-compose-help">{t("Чтобы загрузить файлы, нужен daemon с поддержкой вложений. Изображения и документы пока только в черновике.")}</p>}
      <div className="terminal-compose-actions"><p>{mobile ? t(plainTerminal ? 'Enter — новая строка. «Отправить» передаёт текст в терминал.' : 'Enter — новая строка. «Отправить задачу» вставляет текст и отправляет его агенту.') : t("Enter и Shift+Enter — новая строка. Ctrl/⌘+Enter — вставить черновик.")}<br />{t("Файлы копируются в папку проекта; в CLI вставляются пути. Отправьте задачу после проверки.")}</p>{mobile && voiceInput}{mobile && <button type="button" className="terminal-input-more" aria-label={t(open ? 'Свернуть поле ввода' : 'Развернуть ввод и вложения')} aria-expanded={open} onClick={() => { setOpen(value => !value); if (visible) textarea.current?.focus({ preventScroll: true }) }}>{open ? '⌄' : '＋'}{files.length > 0 && <small>{files.length}</small>}</button>}{mobile && <button type="button" className="terminal-paste-only" disabled={busy || voiceActive || !running || !visible || (!draft && !files.length) || (files.length > 0 && !attachmentsAvailable)} onClick={() => void insert()}>{t('Вставить в терминал')}</button>}<button type="button" className="terminal-message-send" onPointerDown={event => { if (mobile && document.activeElement === textarea.current) event.preventDefault() }} aria-label={mobile ? t(plainTerminal ? "Отправить в терминал" : "Отправить сообщение") : undefined} disabled={busy || voiceActive || !running || !visible || (!draft && !files.length) || (files.length > 0 && !attachmentsAvailable)} onClick={() => { if (mobile) textarea.current?.focus({ preventScroll: true }); void insert(mobile && !!onSubmit) }}>{busy ? t("Подготавливаем…") : mobile ? t(open && !plainTerminal ? 'Отправить задачу' : 'Отправить') : t("Вставить в терминал")}</button></div>
      {!running && <p className="terminal-compose-help">{t("Сессия завершена. Черновик сохранён, вставка и загрузка файлов отключены.")}</p>}
    </div>
    {lastPaths.length > 0 && <details className="terminal-inserted-files"><summary>{t("Файлы в проекте:")}{' '}{lastPaths.length}</summary>{lastPaths.map(path => <code key={path}>{path}</code>)}</details>}
  </div>
})

export default TerminalComposer
