import { api } from './api'
import { t } from './i18n'
import { voiceErrors } from './locales/voice'
import { mobileClient, mobileInvoke } from './native-mobile'

export type VoiceProvider = 'openai' | 'openrouter' | 'groq' | 'custom'

export type VoiceProviderSettings = {
  id: VoiceProvider
  base_url: string
  model: string
  has_key: boolean
}

export type VoiceSettings = {
  provider: VoiceProvider
  providers: VoiceProviderSettings[]
  enabled: boolean
  base_url: string
  model: string
  has_key: boolean
  revision: string
  capture: 'native' | 'web'
}

export function voiceMessage(code: string): string {
  return t(voiceErrors[code] || voiceErrors.voice_unavailable)
}

export function voiceAvailable(): boolean {
  return mobileClient() || window.location.protocol === 'mpac:'
}

export async function voiceRequest<T>(token: string, operation: string, body?: unknown): Promise<T> {
  try {
    if (mobileClient()) {
      return await mobileInvoke<T>('voice_request', {
        method: body === undefined ? 'GET' : 'POST',
        operation,
        body: body === undefined ? null : JSON.stringify(body),
      })
    }
    return await api<T>(token, '/native/voice/' + operation, body)
  } catch (error) {
    const message = typeof error === 'string' ? error : error instanceof Error ? error.message : ''
    throw new Error(voiceMessage(message))
  }
}

const MAX_AUDIO = 8 * 1024 * 1024
export const RECORD_SECONDS = 60

export async function webRecording(signal: AbortSignal) {
  if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === 'undefined') {
    throw new Error(voiceMessage('voice_unavailable'))
  }
  let stream: MediaStream
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1 } })
  } catch (error) {
    throw new Error(voiceMessage(error instanceof DOMException && error.name === 'NotAllowedError' ? 'voice_permission' : 'voice_capture'))
  }
  const release = () => stream.getTracks().forEach(track => track.stop())
  if (signal.aborted) {
    release()
    signal.throwIfAborted()
  }
  const mimeType = ['audio/webm;codecs=opus', 'audio/mp4', 'audio/webm'].find(type => MediaRecorder.isTypeSupported(type))
  if (!mimeType) {
    release()
    throw new Error(voiceMessage('voice_audio_format'))
  }

  try {
    const recorder = new MediaRecorder(stream, { mimeType, audioBitsPerSecond: 48000 })
    const chunks: Blob[] = []
    let size = 0
    let timer: ReturnType<typeof setTimeout>
    const stop = () => {
      if (recorder.state !== 'inactive') recorder.stop()
    }
    const cancel = () => { stop(); release() }
    const audio = new Promise<Blob>((resolve, reject) => {
      const cleanup = () => {
        clearTimeout(timer)
        signal.removeEventListener('abort', cancel)
        release()
      }
      recorder.ondataavailable = ({ data }) => {
        size += data.size
        if (size > MAX_AUDIO) {
          stop()
          return
        }
        chunks.push(data)
      }
      recorder.onstop = () => {
        cleanup()
        if (signal.aborted) reject(signal.reason)
        else if (size > MAX_AUDIO) reject(new Error(voiceMessage('voice_audio_size')))
        else if (size === 0) reject(new Error(voiceMessage('voice_no_audio')))
        else resolve(new Blob(chunks, { type: recorder.mimeType || mimeType }))
      }
      recorder.onerror = () => {
        cleanup()
        reject(new Error(voiceMessage('voice_capture')))
      }
      try {
        recorder.start(1000)
        timer = setTimeout(stop, RECORD_SECONDS * 1000)
        signal.addEventListener('abort', cancel, { once: true })
      } catch {
        cleanup()
        reject(new Error(voiceMessage('voice_capture')))
      }
    })
    // A device failure can finish the recording before the user presses Stop.
    void audio.catch(() => {})
    return () => {
      stop()
      return audio
    }
  } catch (error) {
    release()
    throw error
  }
}
