import { useEffect, useRef, useState } from 'react'
import { t } from './i18n'
import { attachmentBase64 } from './terminal-input'
import { RECORD_SECONDS, voiceMessage, voiceRequest, webRecording } from './voice'
import type { VoiceSettings } from './voice'

type Operation = {
  id: string
  revision: string
  controller: AbortController
  finish?: () => Promise<{ text: string }>
}

export default function VoiceInput({ token, settings, disabled, onActive, onError, onText }: {
  token: string
  settings: VoiceSettings
  disabled: boolean
  onActive: (active: boolean) => void
  onError: (message: string) => void
  onText: (text: string) => void
}) {
  const operation = useRef<Operation | null>(null)
  const [phase, setPhase] = useState<'idle' | 'permission' | 'recording' | 'transcribing'>('idle')
  const [elapsed, setElapsed] = useState(0)

  function release(current: Operation) {
    current.controller.abort()
    if (settings.capture === 'native') {
      const { id, revision } = current
      void voiceRequest(token, 'cancel', { id, revision }).catch(() => {})
    }
  }

  function cancel() {
    const current = operation.current
    operation.current = null
    if (current) release(current)
    onActive(false)
    setPhase('idle')
  }

  useEffect(() => {
    const hidden = () => { if (document.hidden) cancel() }
    document.addEventListener('visibilitychange', hidden)
    return () => {
      document.removeEventListener('visibilitychange', hidden)
      const current = operation.current
      operation.current = null
      if (current) release(current)
      onActive(false)
    }
  }, [token, settings.revision])

  useEffect(() => {
    if (phase !== 'recording') return
    const startedAt = Date.now()
    const timer = setInterval(() => setElapsed(Math.min(RECORD_SECONDS, Math.floor((Date.now() - startedAt) / 1000))), 250)
    return () => clearInterval(timer)
  }, [phase])

  async function begin() {
    if (disabled || operation.current) return
    // getRandomValues is available in the desktop custom scheme too.
    const random = crypto.getRandomValues(new Uint8Array(16))
    const hex = Array.from(random, byte => byte.toString(16).padStart(2, '0')).join('')
    const id = [hex.slice(0, 8), hex.slice(8, 12), hex.slice(12, 16), hex.slice(16, 20), hex.slice(20)].join('-')
    const current: Operation = { id, revision: settings.revision, controller: new AbortController() }
    operation.current = current
    onActive(true)
    onError('')
    setElapsed(0)
    setPhase('permission')
    try {
      if (settings.capture === 'native') {
        const lease = { id, revision: current.revision }
        await voiceRequest(token, 'start', lease)
        current.finish = () => voiceRequest(token, 'stop', lease)
      } else {
        const signal = current.controller.signal
        const stop = await webRecording(signal)
        current.finish = async () => {
          const audio = await stop()
          const data_base64 = await attachmentBase64(audio, signal)
          signal.throwIfAborted()
          return voiceRequest(token, 'transcribe', { revision: current.revision, mime_type: audio.type, data_base64 })
        }
      }
      if (operation.current !== current) { release(current); return }
      setPhase('recording')
    } catch (error) {
      if (operation.current === current) {
        cancel()
        onError(error instanceof Error ? error.message : voiceMessage('voice_capture'))
      }
    }
  }

  async function finish() {
    const current = operation.current
    const transcribe = current?.finish
    if (!current || !transcribe) return
    current.finish = undefined
    setPhase('transcribing')
    try {
      const result = await transcribe()
      if (operation.current === current) onText(result.text)
    } catch (error) {
      if (operation.current === current) onError(error instanceof Error ? error.message : voiceMessage('voice_response'))
    } finally {
      if (operation.current === current) cancel()
    }
  }

  return <div className="voice-controls">
    {phase === 'idle' ? <button type="button" className="voice-button" disabled={disabled} aria-label={t('Записать голос')} title={t('Записать голос')} onClick={() => void begin()}>
      <svg aria-hidden="true" viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.7"><rect x="9" y="3" width="6" height="12" rx="3" /><path d="M5 11v1a7 7 0 0 0 14 0v-1M12 19v3m-3 0h6" /></svg>
    </button> : <>
      {phase === 'recording' ? <button type="button" className="voice-recording" onClick={() => void finish()} aria-label={t('Остановить и распознать')} title={t('Остановить и распознать')}><span aria-hidden="true">■</span>{' '}{elapsed === RECORD_SECONDS ? t('Распознать') : t('{p0} / {p1} с', { p0: elapsed, p1: RECORD_SECONDS })}</button> : <span role="status">{t(phase === 'permission' ? 'Микрофон…' : 'Распознаём…')}</span>}
      <button type="button" className="voice-button" aria-label={t('Отменить голосовой ввод')} title={t('Отменить голосовой ввод')} onClick={cancel}>×</button>
    </>}
  </div>
}
