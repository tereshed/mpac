/** The installed mobile shell supplies this flag before bundled scripts run.
 * Ordinary browsers and the desktop client retain their existing transport. */
type NativeWindow = Window & { __MPAC_NATIVE_MOBILE__?: boolean; __TAURI__?: { core: { invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T> } } };
export function mobileClient(): boolean { return typeof window !== 'undefined' && (window as NativeWindow).__MPAC_NATIVE_MOBILE__ === true }
export function mobileInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const native = typeof window === 'undefined' ? undefined : (window as NativeWindow).__TAURI__
  if (!mobileClient() || !native) return Promise.reject(new Error('Mobile native connection unavailable'))
  return native.core.invoke<T>(command, args)
}
export async function forgetMobileConnection(): Promise<void> {
  await mobileInvoke('mobile_forget')
  window.dispatchEvent(new Event('mpac-mobile-disconnected'))
}
export type IncomingConnection = {kind: 'pair'; link: string; endpoint: string} | {kind: 'invalid'}
export function nextIncomingConnection(current: IncomingConnection | null, next: IncomingConnection): IncomingConnection {
  return next.kind === 'invalid' && current?.kind === 'pair' ? current : next
}

/** Cancellation never retries a request and cannot leave the review UI busy forever. */
export function cancelMobilePairing(timeoutMs = 5000): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined
  return Promise.race([
    mobileInvoke<void>('mobile_cancel'),
    new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error('Pairing cancellation timed out')), timeoutMs) }),
  ]).finally(() => { if (timer !== undefined) clearTimeout(timer) })
}
