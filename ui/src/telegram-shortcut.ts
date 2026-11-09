import { api } from './api.ts'

/** One explicit click, one delivery attempt; failures never retry automatically. */
export function telegramLinkSender(token: string, request = (body: { request_id: string }) => api<{sent: boolean}>(token, '/native/phone/send-link', body)) {
  let sending = false
  return async (): Promise<boolean> => {
    if (sending) return false
    sending = true
    try {
      const request_id = Array.from(crypto.getRandomValues(new Uint8Array(32)), b => b.toString(16).padStart(2, '0')).join('')
      const result = await request({ request_id })
      if (result.sent !== true) throw new Error('Telegram delivery was not confirmed. Check your chat before trying again.')
      return true
    } finally { sending = false }
  }
}
