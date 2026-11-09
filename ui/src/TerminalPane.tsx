import type { VoiceSettings } from './voice'
import { flushSync } from 'react-dom'
import { terminalControlSequence } from './terminal-controls'
import type { TerminalControl } from './terminal-controls'
import { getLocale, t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import '@xterm/xterm/css/xterm.css'
import { ApiError, api, errorMessage, isRunning } from './api'
import type { Session } from './api'
import TerminalComposer from './TerminalComposer'
import type { TerminalComposerHandle } from './TerminalComposer'
import { clipboardFiles, validateTerminalPaste } from './terminal-input'
import { directNewlineSequence, isTerminalNewlineKey } from './terminal-keyboard'
import { findTerminalFileLinks, findTerminalFileLinksInRows, MAX_TERMINAL_LINK_ROWS } from './terminal-file-links'
import ActivitySummary from './ActivitySummary'
import { TerminalInputBatcher, TerminalRenderQueue, TerminalStream } from './terminal-stream.ts'
import type { TerminalOutput } from './terminal-stream.ts'
import { mobileClient } from './native-mobile'
import { TerminalTouchScroll, terminalReadingText, terminalLiveText } from './terminal-mobile'
import AgentAnswerPane from './AgentAnswerPane'
import { applyTerminalOutput } from './terminal-geometry'
import './terminal-mobile.css'

function localizeTerminalAccessibility() {
  Terminal.strings.promptLabel = t('Ввод терминала')
  Terminal.strings.tooMuchOutput = t('Слишком много вывода для чтения. Перейдите к строкам терминала, чтобы прочитать содержимое.')
}

export default function TerminalPane({ token, session, voice, visible, compact = false, autoFocus = true, attachmentsAvailable = false, streamingAvailable = false, answersAvailable = false, resizeAvailable = true, onOpenFile, onMarkActivityRead }: { token: string; session: Session; voice?: VoiceSettings | null; visible: boolean; compact?: boolean; autoFocus?: boolean; attachmentsAvailable?: boolean; streamingAvailable?: boolean; answersAvailable?: boolean; resizeAvailable?: boolean; onOpenFile?: (reference: string) => void; onMarkActivityRead?: (expectedRevision: number) => Promise<void> }) {
  const locale = getLocale()
  const host = useRef<HTMLDivElement>(null)
  const terminal = useRef<Terminal | null>(null)
  const reader = useRef<HTMLPreElement>(null)
  const readerFollow = useRef(true)
  const refreshReading = useRef<(() => void) | null>(null)
  const geometryReady = useRef(false)
  const fitVisible = useRef<(() => void) | null>(null)
  const hint = useRef<{ element: HTMLDivElement; reference: string; modifier: string } | null>(null)
  const composer = useRef<TerminalComposerHandle>(null)
  const context = session.id + '\0' + token
  const [backgroundOpen, setBackgroundOpen] = useState(false)
  const [focused, setFocused] = useState(false)
  const [coarsePointer, setCoarsePointer] = useState(() => window.matchMedia('(any-pointer: coarse)').matches)
  const mobileControls = compact || mobileClient() || coarsePointer
  const passive = mobileControls || !resizeAvailable
  const [presentation, setPresentation] = useState<'text' | 'screen'>('text')
  const readingMode = passive && presentation === 'text'
  const readingModeRef = useRef(readingMode); readingModeRef.current = readingMode
  const [reading, setReading] = useState('')
  const [geometryKnown, setGeometryKnown] = useState<boolean | null>(null)
  const mobileRef = useRef(mobileControls); mobileRef.current = mobileControls
  const [history, setHistory] = useState<string | null>(null)
  const [answerContext, setAnswerContext] = useState<string | null>(null)
  const canReadAnswer = answersAvailable && (session.agent === 'claude' || session.agent === 'codex')
  const answerOpen = canReadAnswer && answerContext === context
  const [controlBusy, setControlBusy] = useState(false)
  const [moreKeys, setMoreKeys] = useState(false)
  const controlPending = useRef(false)
  const controlHandler = useRef<{ context: string; input: (data: string) => Promise<void>; newline: () => Promise<void> } | null>(null)
  useEffect(() => { setHistory(null); setControlBusy(false); setReading(''); setGeometryKnown(null); geometryReady.current = false; readerFollow.current = true }, [context, passive])
  useEffect(() => { setAnswerContext(null) }, [context])
  useEffect(() => { if (reader.current && readerFollow.current) reader.current.scrollTop = reader.current.scrollHeight }, [reading])
  useEffect(() => {
    const query = window.matchMedia('(any-pointer: coarse)')
    const update = () => setCoarsePointer(query.matches)
    query.addEventListener('change', update)
    return () => query.removeEventListener('change', update)
  }, [])
  useEffect(() => { setFocused(false) }, [session.id, compact])
  const effectiveVisible = visible && !backgroundOpen && history === null && !answerOpen
  const latestContext = useRef(context)
  const isVisible = useRef(visible)
  const pasteHandler = useRef<{ context: string; paste: (text: string) => Promise<void> } | null>(null)
  const fileOpener = useRef(onOpenFile); fileOpener.current = onOpenFile
  latestContext.current = context
  isVisible.current = effectiveVisible
  const streamCapability = useRef(streamingAvailable); streamCapability.current = streamingAvailable
  const resizeCapability = useRef(resizeAvailable); resizeCapability.current = resizeAvailable
  const running = useRef(isRunning(session))
  const [error, setError] = useState('')
  const [inputError, setInputError] = useState('')
  const [resetNotice, setResetNotice] = useState(false)
  running.current = isRunning(session)

  useEffect(() => {
    const element = host.current!
    const abort = new AbortController()
    let active = true
    const alive = () => active && latestContext.current === context
    let timer: ReturnType<typeof setTimeout>
    let resizeTimer: ReturnType<typeof setTimeout>
    let readingTimer: ReturnType<typeof setTimeout> | undefined
    const rendered = new TerminalRenderQueue()
    let previousSize = ''
    let fitFrame = 0
    let stream: TerminalStream | null = null
    let streamReady: Promise<TerminalStream> | null = null
    const useStream = location.protocol === 'mpac:' || streamCapability.current
    let legacyPolling = false
    let legacyWake = false
    let idleDelay = 16
    let capturedPaste: Promise<void>[] | null = null
    let userInputEvent: Event | null = null
    const markUserInput = (event: Event) => { userInputEvent = event }
    const userInputEvents = ['keydown', 'keypress', 'keyup', 'beforeinput', 'input', 'compositionstart', 'compositionupdate', 'compositionend']
    for (const event of userInputEvents) element.addEventListener(event, markUserInput, true)
    setError('')
    setInputError('')
    setResetNotice(false)
    const mac = /Mac|iPhone|iPad|iPod/i.test(navigator.platform || navigator.userAgent)
    const fileReference = (text: string) => findTerminalFileLinks(text).some(link => link.start === 0 && link.end === text.length)
    const activateFile = (event: MouseEvent, reference: string) => {
      if (!alive() || !isVisible.current || !fileOpener.current || event.button !== 0 || event.altKey || event.shiftKey || (mac ? !event.metaKey || event.ctrlKey : !event.ctrlKey || event.metaKey) || !fileReference(reference)) return
      event.preventDefault(); event.stopPropagation()
      term.clearSelection()
      try { fileOpener.current(reference) } catch (failure) { if (alive()) setInputError(errorMessage(failure)) }
    }
    const term = new Terminal({
      cursorBlink: true, fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      fontSize: 13, lineHeight: 1.35, scrollback: 5000, convertEol: false,
      allowProposedApi: false, theme: {
        background: '#0c1016', foreground: '#d7dfeb', cursor: '#a4edb7',
        selectionBackground: '#33485b', black: '#18212b', brightBlack: '#697d8f',
        red: '#ff8383', green: '#a4edb7', yellow: '#f4ce8c', blue: '#9dc7ff',
        magenta: '#d4adff', cyan: '#90e4df', white: '#e5ecf5',
      },
      // Override OSC8 activation too: no terminal hyperlink opens an external
      // browser or executes a scheme. Only modified local references navigate.
      linkHandler: { allowNonHttpProtocols: true, activate: activateFile },
    })
    terminal.current = term
    const fit = new FitAddon()
    term.loadAddon(fit)
    localizeTerminalAccessibility()
    term.open(element)
    const linkHint = document.createElement('div')
    linkHint.className = 'terminal-link-tooltip xterm-hover'
    linkHint.hidden = true
    element.append(linkHint)
    const hideHint = () => { linkHint.hidden = true }
    const showHint = (event: MouseEvent, text: string) => {
      if (!alive() || !isVisible.current || !fileOpener.current || !fileReference(text)) { hideHint(); return }
      const bounds = element.getBoundingClientRect()
      hint.current = { element: linkHint, reference: text, modifier: mac ? '⌘' : 'Ctrl' }
      linkHint.textContent = t("{p0} + клик — открыть файл\n{p1}", { p0: hint.current.modifier, p1: text })
      linkHint.style.left = Math.max(4, Math.min(bounds.width - 180, event.clientX - bounds.left + 8)) + 'px'
      linkHint.style.top = Math.max(4, event.clientY - bounds.top + 20) + 'px'
      linkHint.hidden = false
    }
    term.options.linkHandler = { allowNonHttpProtocols: true, activate: activateFile, hover: showHint, leave: hideHint }
    const links = term.registerLinkProvider({
      provideLinks(bufferLineNumber, callback) {
        if (!alive() || !fileOpener.current) { callback(undefined); return }
        const buffer = term.buffer.active
        let start = bufferLineNumber - 1
        while (start > 0 && buffer.getLine(start)?.isWrapped && bufferLineNumber - start < MAX_TERMINAL_LINK_ROWS) start--
        const first = buffer.getLine(start)
        if (!first || first.isWrapped) { callback(undefined); return }
        const rows = [first]
        let next = start + 1
        while (rows.length < MAX_TERMINAL_LINK_ROWS && buffer.getLine(next)?.isWrapped) { rows.push(buffer.getLine(next)!); next++ }
        if (buffer.getLine(next)?.isWrapped) { callback(undefined); return }
        callback(findTerminalFileLinksInRows(rows, start + 1).filter(link => link.range.start.y <= bufferLineNumber && link.range.end.y >= bufferLineNumber).map(link => ({
          ...link, activate: activateFile, hover: showHint, leave: hideHint,
        })))
      },
    })
    term.options.disableStdin = !running.current

    const syncPassiveSurface = () => {
      if (!passive) return
      const screen = element.querySelector<HTMLElement>('.xterm-screen')
      if (!screen) return
      const bounds = screen.getBoundingClientRect()
      const padding = getComputedStyle(element)
      element.style.width = `${Math.ceil(bounds.width + parseFloat(padding.paddingLeft) + parseFloat(padding.paddingRight))}px`
      element.style.height = `${Math.ceil(bounds.height + parseFloat(padding.paddingTop) + parseFloat(padding.paddingBottom))}px`
      element.dataset.ptyCols = String(term.cols)
      element.dataset.ptyRows = String(term.rows)
    }
    const captureReading = () => {
      if (!alive()) return
      setReading(terminalLiveText(term.buffer.active, term.rows))
    }
    refreshReading.current = captureReading
    const scheduleReading = () => {
      if (readingTimer || !readingModeRef.current) return
      readingTimer = setTimeout(() => { readingTimer = undefined; if (alive() && isVisible.current) captureReading() }, 200)
    }

    const fitTerminal = () => {
      if (!alive() || !isVisible.current || element.clientWidth < 50 || element.clientHeight < 40) return
      if (passive) { syncPassiveSurface(); return }
      const dimensions = fit.proposeDimensions()
      if (!dimensions) return
      const cols = Math.max(20, Math.min(300, dimensions.cols))
      const rows = Math.max(5, Math.min(150, dimensions.rows))
      if (term.cols !== cols || term.rows !== rows) {
        const offset = term.buffer.active.baseY - term.buffer.active.viewportY
        term.resize(cols, rows)
        if (offset > 0) term.scrollToLine(Math.max(0, term.buffer.active.baseY - offset))
      }
      const size = `${term.cols}x${term.rows}`
      if (size === previousSize) return
      previousSize = size
      clearTimeout(resizeTimer)
      resizeTimer = setTimeout(() => {
        if (!running.current || !alive() || !resizeCapability.current) return
        const connection = streamReady
        const operation = useStream
          ? connection?.then(channel => { if (alive() && streamReady === connection) return channel.resize(term.cols, term.rows) })
          : api(token, `/sessions/${session.id}/resize`, { cols: term.cols, rows: term.rows }, abort.signal)
        void operation?.catch(e => { if (alive() && !abort.signal.aborted) setError(errorMessage(e)) })
      }, 100)
    }
    const scheduleFit = () => { if (!fitFrame) fitFrame = requestAnimationFrame(() => { fitFrame = 0; fitTerminal() }) }
    fitVisible.current = scheduleFit
    const observer = new ResizeObserver(scheduleFit)
    observer.observe(element)
    scheduleFit()
    void document.fonts?.ready.then(() => { if (alive()) scheduleFit() })

    const inputBatcher = new TerminalInputBatcher(async data => {
      if (!alive() || !running.current) throw new Error(t("Сессия изменилась или завершилась. Ввод не отправлен."))
      if (passive && !geometryReady.current) throw new Error(t('Для корректного отображения обновите MPAC на рабочем компьютере.'))
      if (useStream) {
        const connection = streamReady
        if (!connection) throw new Error(t('Поток терминала не готов. Ввод не отправлен.'))
        const channel = await connection
        if (!alive() || streamReady !== connection) throw new Error(t('Поток терминала не готов. Ввод не отправлен.'))
        await channel.input(data)
      } else {
        await api(token, `/sessions/${session.id}/input`, { data }, abort.signal)
        wakeLegacy()
      }
    })
    const enqueueInput = (data: string): Promise<void> => {
      if (!alive() || !running.current) return Promise.reject(new Error(t("Сессия больше не принимает ввод. Черновик не отправлен.")))
      return inputBatcher.enqueue(data)
    }
    const input = term.onData(data => {
      if (!data || !alive() || !running.current) return
      // A mirror must not answer historical cursor/device queries back into a
      // live agent. Replies generated by xterm start with ESC. Keyboard escape
      // sequences are accepted only during DOM dispatch or an explicit paste;
      // printable IME input may arrive asynchronously after compositionend.
      if (passive && data.startsWith('\x1b') && !userInputEvent?.eventPhase && capturedPaste === null) return
      const operation = enqueueInput(data)
      capturedPaste?.push(operation)
      void operation.catch(e => { if (alive() && !abort.signal.aborted) setInputError(t("Ввод не подтверждён: {p0} Проверьте терминал перед повтором.", { p0: errorMessage(e) })) })
    })

    const paste = async (text: string) => {
      if (!alive() || !running.current || !isVisible.current) throw new Error(t("Вернитесь в терминал перед вставкой."))
      validateTerminalPaste(text, term.modes.bracketedPasteMode)
      const operations: Promise<void>[] = []
      capturedPaste = operations
      try { term.paste(text) }
      finally { capturedPaste = null }
      if (!operations.length) throw new Error(t("CLI не принял вставку. Черновик сохранён."))
      await Promise.all(operations)
    }
    pasteHandler.current = { context, paste }
    const newline = async () => {
      const sequence = directNewlineSequence(session.agent)
      if (sequence) await enqueueInput(sequence)
      else if (term.modes.bracketedPasteMode) await paste('\n')
      else throw new Error(t("Этот CLI не поддерживает прямой Shift+Enter. Используйте многострочное поле «Черновик». Enter не отправлен."))
    }
    controlHandler.current = { context, input: enqueueInput, newline }
    let lastEnterKey = -Infinity
    term.attachCustomKeyEventHandler(event => {
      if (mobileRef.current && event.key === 'Enter' && !event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing && event.keyCode !== 229) {
        event.preventDefault(); event.stopPropagation()
        if (event.type === 'keydown' && alive() && running.current && isVisible.current) {
          lastEnterKey = performance.now()
          void enqueueInput('\r').catch(failure => { if (alive()) setInputError(errorMessage(failure)) })
        }
        return false
      }
      if (!isTerminalNewlineKey(event)) return true
      // Suppress xterm's default CR on every event in this key gesture.
      // Only keydown enqueues input; keypress/keyup must not insert it twice.
      event.preventDefault()
      event.stopPropagation()
      if (event.type === 'keydown' && alive() && running.current && isVisible.current) {
        const sequence = directNewlineSequence(session.agent)
        if (sequence) {
          void enqueueInput(sequence).catch(failure => {
            if (alive() && !abort.signal.aborted) setInputError(t("Перенос не подтверждён: {p0} Проверьте строку перед повтором.", { p0: errorMessage(failure) }))
          })
        } else if (term.modes.bracketedPasteMode) {
          void paste('\n').catch(failure => {
            if (alive() && !abort.signal.aborted) setInputError(t("Перенос не подтверждён: {p0}", { p0: errorMessage(failure) }))
          })
        } else {
          setInputError(t("Этот CLI не поддерживает прямой Shift+Enter. Используйте многострочное поле «Черновик». Enter не отправлен."))
        }
      }
      return false
    })
    const clipboardPaste = (event: ClipboardEvent) => {
      if (!alive() || !isVisible.current) return
      const files = clipboardFiles(event.clipboardData)
      const text = event.clipboardData?.getData('text/plain') || ''
      if (!files.length && !text) return
      event.preventDefault(); event.stopPropagation()
      if (files.length) { composer.current?.stage('', files); return }
      try {
        if (!running.current) throw new Error(t("Сессия завершена. Текст оставлен в черновике."))
        validateTerminalPaste(text, term.modes.bracketedPasteMode)
      } catch (failure) { composer.current?.stage(text, [], errorMessage(failure)); return }
      void paste(text).catch(failure => { if (alive()) composer.current?.stage(text, [], t("{p0} Проверьте терминал перед повторной вставкой.", { p0: errorMessage(failure) })) })
    }
    element.addEventListener('paste', clipboardPaste, true)
    // iOS/Android keyboards may emit beforeinput instead of a usable Enter key.
    const mobileLineBreak = (event: InputEvent) => {
      if (!mobileRef.current || event.isComposing || !['insertLineBreak', 'insertParagraph'].includes(event.inputType)) return
      event.preventDefault(); event.stopImmediatePropagation()
      if (performance.now() - lastEnterKey < 100 || !alive() || !running.current || !isVisible.current) return
      void enqueueInput('\r').catch(failure => { if (alive()) setInputError(errorMessage(failure)) })
    }
    term.textarea?.addEventListener('beforeinput', mobileLineBreak, true)
    const touch = new TerminalTouchScroll()
    const touchStart = (event: TouchEvent) => {
      if (passive || !mobileRef.current || event.touches.length !== 1) { touch.end(); return }
      touch.start(event.touches[0].clientX, event.touches[0].clientY)
    }
    const touchMove = (event: TouchEvent) => {
      if (passive || !mobileRef.current || !alive() || !isVisible.current || event.touches.length !== 1) { touch.end(); return }
      const height = (element.querySelector('.xterm-screen')?.getBoundingClientRect().height || element.clientHeight) / term.rows
      const movement = touch.move(event.touches[0].clientX, event.touches[0].clientY, height)
      if (!movement.scrolling) return
      if (event.cancelable) event.preventDefault()
      event.stopPropagation()
      if (movement.lines) term.scrollLines(movement.lines)
    }
    const touchEnd = () => touch.end()
    element.addEventListener('touchstart', touchStart, { passive: true })
    element.addEventListener('touchmove', touchMove, { passive: false })
    element.addEventListener('touchend', touchEnd)
    element.addEventListener('touchcancel', touchEnd)

    const renderOutput = (output: TerminalOutput) => rendered.write(output, async result => {
      if (!alive()) return
      if (result.reset) { term.reset(); setResetNotice(true) }
      const previous = `${term.cols}x${term.rows}`
      const supported = await applyTerminalOutput(result, term, passive)
      if (!alive()) return
      if (passive) {
        geometryReady.current = supported
        setGeometryKnown(supported)
        term.options.disableStdin = !supported || !running.current
        syncPassiveSurface()
        if (supported && (result.data_base64 || result.reset || previous !== `${term.cols}x${term.rows}`)) scheduleReading()
      }
    })
    const reconnect = (failure: Error) => {
      if (!alive()) return
      stream = null; streamReady = null
      inputBatcher.cancel(failure)
      setError(errorMessage(failure))
      clearTimeout(timer)
      timer = setTimeout(connectStream, 500)
    }
    const connectStream = () => {
      if (!alive()) return
      const opening = Promise.resolve().then(async () => {
        let address: string
        if (location.protocol === 'mpac:') {
          const bridge = await api<{ url: string }>(token, '/native/terminal', undefined, abort.signal)
          const url = new URL(bridge.url)
          if (url.protocol !== 'ws:' || url.hostname !== '127.0.0.1' || !url.port || url.pathname !== '/terminal' || url.search || url.hash || url.username || url.password) throw new Error(t('Не удалось открыть поток терминала.'))
          address = url.href
        } else {
          const url = new URL(`/api/sessions/${encodeURIComponent(session.id)}/terminal`, location.href)
          url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'
          address = url.href
        }
        const after = await rendered.cursor()
        if (!alive() || streamReady !== opening) throw new Error(t('Поток терминала закрыт.'))
        const channel = new TerminalStream({
          url: address, token, sessionId: session.id, after, native: location.protocol === 'mpac:',
          onOutput: renderOutput,
          onDisconnect: failure => { if (alive() && stream === channel) reconnect(failure) },
          onEnd: () => {
            if (!alive() || stream !== channel) return
            streamReady = null
            inputBatcher.cancel(new Error(t('Поток терминала закрыт.')))
            setError('')
          },
        })
        stream = channel
        await channel.ready
        if (alive() && stream === channel) { setError(''); previousSize = ''; scheduleFit() }
        return channel
      })
      streamReady = opening
      void opening.catch(failure => { if (alive() && streamReady === opening) reconnect(failure instanceof Error ? failure : new Error(errorMessage(failure))) })
    }
    const poll = async () => {
      if (!alive() || legacyPolling) return
      legacyPolling = true; legacyWake = false
      let bytesRead = 0
      try {
        const result = await api<TerminalOutput>(token, `/sessions/${session.id}/output?after=${rendered.sequence}`, undefined, abort.signal)
        if (!alive()) return
        bytesRead = result.data_base64.length * 0.75
        await renderOutput(result)
        if (!alive()) return
        setError('')
      } catch (e) {
        if (alive() && e instanceof ApiError && e.status === 400) {
          setError(t("Исполнение недоступно: сессия сохранена в истории после перезапуска daemon. Запустите новую сессию для продолжения."))
          return
        }
        if (alive() && !abort.signal.aborted) setError(errorMessage(e))
      } finally { legacyPolling = false }
      idleDelay = bytesRead ? 16 : Math.min(120, idleDelay + 16)
      if (alive()) timer = setTimeout(poll, legacyWake || bytesRead >= 32 * 1024 ? 0 : running.current ? idleDelay : 750)
    }
    function wakeLegacy() {
      if (!alive() || useStream) return
      idleDelay = 16; clearTimeout(timer)
      if (legacyPolling) legacyWake = true
      else void poll()
    }
    if (useStream) connectStream()
    else void poll()
    return () => {
      active = false
      abort.abort()
      clearTimeout(timer)
      clearTimeout(resizeTimer)
      clearTimeout(readingTimer)
      cancelAnimationFrame(fitFrame)
      fitVisible.current = null
      inputBatcher.dispose()
      stream?.close()
      observer.disconnect()
      element.removeEventListener('paste', clipboardPaste, true)
      for (const event of userInputEvents) element.removeEventListener(event, markUserInput, true)
      term.textarea?.removeEventListener('beforeinput', mobileLineBreak, true)
      element.removeEventListener('touchstart', touchStart)
      element.removeEventListener('touchmove', touchMove)
      element.removeEventListener('touchend', touchEnd)
      element.removeEventListener('touchcancel', touchEnd)
      input.dispose()
      links.dispose()
      linkHint.remove()
      if (hint.current?.element === linkHint) hint.current = null
      term.dispose()
      terminal.current = null
      refreshReading.current = null
      if (pasteHandler.current?.context === context) pasteHandler.current = null
      if (controlHandler.current?.context === context) controlHandler.current = null
    }
  }, [token, session.id, passive])

  useEffect(() => {
    localizeTerminalAccessibility()
    terminal.current?.textarea?.setAttribute('aria-label', t('Ввод терминала'))
    if (hint.current && !hint.current.element.hidden) hint.current.element.textContent = t('{p0} + клик — открыть файл\n{p1}', { p0: hint.current.modifier, p1: hint.current.reference })
  }, [locale])

  useEffect(() => {
    if (terminal.current) terminal.current.options.disableStdin = !isRunning(session) || !effectiveVisible || (passive && !geometryReady.current)
  }, [session.status, effectiveVisible, session.id, passive, geometryKnown])

  useEffect(() => { if (effectiveVisible) { fitVisible.current?.(); if (readingMode) { terminal.current?.blur(); refreshReading.current?.() } } }, [effectiveVisible, session.id, readingMode])

  useEffect(() => {
    if (autoFocus && effectiveVisible && !composer.current?.focusIfOpen() && !readingMode) terminal.current?.focus()
  }, [autoFocus, effectiveVisible, session.id, readingMode])

  const control = async (action: TerminalControl) => {
    if (controlPending.current) throw new Error(t('Дождитесь подтверждения предыдущей клавиши.'))
    const handler = controlHandler.current
    if (!handler || handler.context !== context || !running.current || !isVisible.current) {
      const failure = new Error(t('Терминал ещё не готов к вводу.'))
      setInputError(failure.message)
      throw failure
    }
    controlPending.current = true; setControlBusy(true); setInputError('')
    try { await (action === 'newline' ? handler.newline() : handler.input(terminalControlSequence(action, terminal.current?.modes.applicationCursorKeysMode))) }
    catch (failure) {
      if (latestContext.current === context) setInputError(t('Отправка не подтверждена. Проверьте терминал перед повтором: {p0}', { p0: errorMessage(failure) }))
      throw failure
    } finally { controlPending.current = false; if (latestContext.current === context) setControlBusy(false) }
  }
  const readHistory = () => {
    const term = terminal.current
    if (!term) return
    composer.current?.blurInput()
    term.blur()
    const normal = terminalReadingText(term.buffer.normal)
    const active = term.buffer.active.type === 'alternate' ? terminalReadingText(term.buffer.active) : ''
    setHistory([normal, active].filter(Boolean).join('\n\n'))
  }

  return <div className={`terminal-layout ${compact && focused ? 'mobile-terminal-focused' : ''}`} onDragOver={event => { if (event.dataTransfer.types.includes('Files')) event.preventDefault() }} onDrop={event => { const files = clipboardFiles(event.dataTransfer); if (files.length) { event.preventDefault(); event.stopPropagation(); composer.current?.stage('', files) } }}>
    <ActivitySummary key={`activity:${context}`} session={session} compact={compact} dense={!compact} expanded={focused} onToggleExpanded={() => setFocused(value => !value)} onMarkRead={onMarkActivityRead} onBackgroundOpenChange={setBackgroundOpen} tools={<>
      {resetNotice && <span className="terminal-history-indicator" title={t('Часть старого вывода больше недоступна.')} aria-label={t('Часть старого вывода больше недоступна.')} role="img">⚠</span>}
      {canReadAnswer && <button className="terminal-answer-toggle" type="button" aria-pressed={answerOpen} onClick={() => {
        if (answerOpen) { setAnswerContext(null); return }
        terminal.current?.blur()
        composer.current?.blurInput()
        setHistory(null); setAnswerContext(context)
      }}>{t('Последний ответ')}</button>}
    </>} />
    <div className="terminal-caption terminal-caption-hidden"><span><i className="tiny-dot" />{t('PTY-терминал')}</span><span>{t("Shift+Enter — новая строка · Enter — отправить")}{onOpenFile ? t(" · ⌘/Ctrl + клик — файл") : ''}</span></div>
    {passive && !answerOpen && <div className="terminal-presentation" aria-label={t('Вид терминала')}>
      <button type="button" aria-pressed={readingMode} onClick={() => setPresentation('text')}>{t('Текст')}</button>
      <button type="button" aria-pressed={!readingMode} onClick={() => setPresentation('screen')}>{t('Экран CLI')}</button>
      {!readingMode && <span>{t('Проведите по экрану для прокрутки')}</span>}
    </div>}
    {!answerOpen && error && <div className="inline-error" role="alert">{t(error)}</div>}
    {!answerOpen && inputError && <div className="inline-error input-error" role="alert"><span>{t(inputError)}</span><button className="icon-button" onClick={() => setInputError('')} aria-label={t("Закрыть ошибку ввода")}>×</button></div>}
    {!answerOpen && passive && geometryKnown === false && <div className="inline-error" role="alert">{t('Для корректного отображения обновите MPAC на рабочем компьютере.')}</div>}
    <div className={`terminal-surface ${passive ? 'terminal-passive' : ''} ${readingMode ? 'terminal-reading' : ''} ${history !== null || answerOpen ? 'terminal-history-hidden' : ''}`}>
      <div className={`terminal-host ${mobileControls ? 'terminal-touch' : ''}`} ref={host} aria-hidden={history !== null || answerOpen || readingMode} aria-label={t("Терминал: {p0}", { p0: session.title })} />
      {readingMode && <pre ref={reader} className="terminal-reading-text" tabIndex={0} aria-label={t('Текст терминала')} onScroll={event => { const view = event.currentTarget; readerFollow.current = view.scrollHeight - view.clientHeight - view.scrollTop < 48 }}>{geometryKnown === false ? '' : reading || t('Пока нет вывода.')}</pre>}
    </div>
    {history !== null && <section className="terminal-history" aria-label={t('История вывода')}><p>{t('Снимок доступного вывода. Новые сообщения не сдвигают текст; полноэкранный CLI может хранить меньше истории.')} <button type="button" onClick={readHistory}>{t('Обновить снимок')}</button></p><pre tabIndex={0}>{history || t('Пока нет вывода.')}</pre></section>}
    {answerOpen && <AgentAnswerPane key={`answer:${context}`} token={token} sessionId={session.id} onClose={() => setAnswerContext(null)} />}
    {mobileControls && !answerOpen && <div className="terminal-keypad" aria-label={t('Клавиши CLI')}>
      {moreKeys && <div className="terminal-extra-keys">
        <p>{t('В списке: ↑ ↓ — выбрать пункт, Enter — подтвердить. Space — отметить чекбокс.')}</p>
        <div>{([['left', '←', 'Влево'], ['right', '→', 'Вправо'], ['tab', 'Tab', 'Tab — следующее поле'], ['space', 'Space', 'Space — отметить пункт'], ['escape', 'Esc', 'Esc — назад'], ['backspace', '⌫', 'Backspace'], ['newline', '↵', 'Новая строка']] as const).map(([action, label, hint]) => <button key={action} type="button" aria-label={t(hint)} title={t(hint)} disabled={controlBusy || !isRunning(session) || !effectiveVisible || geometryKnown !== true} onClick={() => void control(action).catch(() => {})}>{label}</button>)}<button type="button" aria-label={t('К последнему выводу')} onClick={() => { setHistory(null); readerFollow.current = true; terminal.current?.scrollToBottom(); if (reader.current) reader.current.scrollTop = reader.current.scrollHeight }}>↓•</button></div>
      </div>}
      <div className="terminal-mobile-controls">
        <button type="button" aria-pressed={history !== null} title={t('История')} aria-label={t('История')} onClick={() => { if (history === null) readHistory(); else setHistory(null) }}>◷</button>
        <button type="button" aria-label={t('Показать клавиатуру')} title={t('Показать клавиатуру')} disabled={!visible} onClick={() => { if (history !== null) flushSync(() => setHistory(null)); composer.current?.focusInput() }}>⌨</button>
        {(['up', 'down'] as const).map(action => <button type="button" key={action} aria-label={t(action === 'up' ? 'Предыдущий пункт списка' : 'Следующий пункт списка')} title={t(action === 'up' ? 'Предыдущий пункт списка' : 'Следующий пункт списка')} disabled={controlBusy || !isRunning(session) || !effectiveVisible || geometryKnown !== true} onClick={() => void control(action).catch(() => {})}>{action === 'up' ? '↑' : '↓'}</button>)}
        <button type="button" aria-expanded={moreKeys} aria-label={t('Другие клавиши CLI')} title={t('Другие клавиши CLI')} onClick={() => setMoreKeys(value => !value)}>⋯</button>
        <button type="button" aria-label={t('Отправить Enter в терминал')} title={t('Enter — подтвердить выбор или отправить строку CLI')} className="terminal-mobile-send" disabled={controlBusy || !isRunning(session) || !effectiveVisible || geometryKnown !== true} onClick={() => void control('submit').catch(() => {})}>Enter ↵</button>
      </div>
    </div>}
    <TerminalComposer key={context} ref={composer} token={token} voice={voice} sessionId={session.id} running={isRunning(session) && (!passive || geometryKnown === true)} visible={effectiveVisible} hidden={history !== null || answerOpen} autoFocus={autoFocus} attachmentsAvailable={attachmentsAvailable} mobile={mobileControls} plainTerminal={session.agent === "shell" || session.agent === "command"} onSubmit={() => control('submit')} bracketedPaste={() => latestContext.current === context && terminal.current?.modes.bracketedPasteMode === true} onInsert={text => {
      if (pasteHandler.current?.context !== context) return Promise.reject(new Error(t("Терминал ещё не готов к вставке. Черновик сохранён.")))
      return pasteHandler.current.paste(text)
    }} onFocusTerminal={() => { if (latestContext.current === context && isVisible.current && !readingMode) terminal.current?.focus() }} />
    <div className="terminal-footer"><span>{t("Локальный процесс ·")}{' '}{session.mode === 'interactive_pty' ? 'PTY' : session.mode}</span><span>{isRunning(session) ? t("Ctrl+C — прервать вводимую команду") : t("Сессия завершена · история доступна")}</span></div>
  </div>
}
