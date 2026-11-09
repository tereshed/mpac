import { useEffect, useRef, useState } from 'react'
import App from './App'
import LanguageSwitcher from './LanguageSwitcher'
import useLocale from './useLocale'
import { t } from './i18n'
import { mobileInvoke, nextIncomingConnection, cancelMobilePairing } from './native-mobile'
import type { IncomingConnection as Incoming } from './native-mobile'
import './mobile-client.css'
import Modal from './Modal'

export default function MobileClient() {
  useLocale()
  const [connected, setConnected] = useState(false)
  const [loading, setLoading] = useState(true)
  const [link, setLink] = useState('')
  const [name, setName] = useState('My phone')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [pending, setPending] = useState<{code: string; expires_at: number} | null>(null)
  const [incoming, setIncoming] = useState<Incoming | null>(null)
  const [linkDelivery, setLinkDelivery] = useState<number | null>(null)
  const committedLinkDelivery = useRef<number | null>(null)
  const [returnToWorkspace, setReturnToWorkspace] = useState(false)
  const epoch = useRef(0)
  const mounted = useRef(false)
  useEffect(() => {
    committedLinkDelivery.current = linkDelivery
    if (linkDelivery !== null) void mobileInvoke('mobile_acknowledge_link', {sequence: linkDelivery}).catch(() => {})
  }, [linkDelivery])
  useEffect(() => {
    mounted.current = true
    const generation = ++epoch.current
    void mobileInvoke<{connected: boolean}>('mobile_status').then(status => { if (mounted.current && epoch.current === generation) setConnected(status.connected) })
      .catch(error => { if (mounted.current && epoch.current === generation) setError(typeof error === 'string' ? t(error) : t('Не удалось открыть сохранённое подключение.')) })
      .finally(() => { if (mounted.current) setLoading(false) })
    const lost = () => { ++epoch.current; setConnected(false); setError(t('Доступ телефона отозван. Подключите его заново.')) }
    const disconnected = () => { ++epoch.current; setConnected(false); setPending(null); setError('') }
    window.addEventListener('mpac-mobile-access-lost', lost)
    window.addEventListener('mpac-mobile-disconnected', disconnected)
    return () => { mounted.current = false; ++epoch.current; window.removeEventListener('mpac-mobile-access-lost', lost); window.removeEventListener('mpac-mobile-disconnected', disconnected) }
  }, [])
  useEffect(() => {
    let stopped = false, reading = false
    const receive = async () => {
      if (stopped || reading || document.visibilityState === 'hidden') return
      reading = true
      try {
        const next = await mobileInvoke<((Incoming | {kind: 'open'}) & {sequence: number}) | null>('mobile_take_link')
        if (!stopped && next && Number.isSafeInteger(next.sequence) && next.sequence > 0) {
          if (next.sequence === committedLinkDelivery.current) {
            await mobileInvoke('mobile_acknowledge_link', {sequence: next.sequence}).catch(() => {})
          } else {
            if (next.kind !== 'open') setIncoming(current => nextIncomingConnection(current, next))
            setLinkDelivery(next.sequence)
          }
        }
      } catch { /* Pairing by paste remains available if OS delivery is unavailable. */ }
      finally { reading = false }
    }
    void receive()
    const timer = setInterval(() => { void receive() }, 800)
    const resume = () => { void receive() }
    window.addEventListener('focus', resume); document.addEventListener('visibilitychange', resume)
    return () => { stopped = true; clearInterval(timer); window.removeEventListener('focus', resume); document.removeEventListener('visibilitychange', resume) }
  }, [])
  useEffect(() => {
    if (!pending) return
    const generation = epoch.current
    let cancelled = false, timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      if (cancelled || generation !== epoch.current) return
      if (Date.now() >= pending.expires_at * 1000) { setPending(null); setError(t('Ссылка привязки истекла. Создайте новую на компьютере.')); return }
      try {
        const result = await mobileInvoke<{status: string}>('mobile_pair_status')
        if (cancelled || generation !== epoch.current) return
        if (result.status === 'approved') { setPending(null); setReturnToWorkspace(false); setConnected(true); return }
        if (result.status !== 'pending') { setPending(null); setError(t('Ссылка привязки истекла. Создайте новую на компьютере.')); return }
        setError('')
      } catch { if (!cancelled && generation === epoch.current) setError(t('Компьютер недоступен. Проверяем подключение…')) }
      if (!cancelled && generation === epoch.current) timer = setTimeout(poll, 1500)
    }
    void poll()
    return () => { cancelled = true; clearTimeout(timer) }
  }, [pending])
  const begin = async () => {
    if (busy) return
    const generation = ++epoch.current
    setBusy(true); setError('')
    try {
      const attempt = await mobileInvoke<{code: string; expires_at: number}>('mobile_begin', {link, name})
      if (mounted.current && generation === epoch.current) setPending(attempt)
    } catch { if (mounted.current && generation === epoch.current) setError(t('Не удалось подключить телефон. Проверьте ссылку, сеть и компьютер.')) }
    finally { if (mounted.current && generation === epoch.current) setBusy(false) }
  }
  const cancel = async () => { ++epoch.current; setPending(null); setBusy(false); await cancelMobilePairing().catch(() => {}); if (returnToWorkspace) { setReturnToWorkspace(false); setConnected(true) } }
  const forget = async () => {
    setBusy(true)
    try { await mobileInvoke('mobile_forget'); ++epoch.current; setConnected(false); setReturnToWorkspace(false); setPending(null); setError('') }
    catch { setError(t('Не удалось удалить сохранённое подключение.')) }
    finally { setBusy(false) }
  }
  const reviewIncoming = async () => {
    if (!incoming || incoming.kind !== 'pair' || busy || pending || loading) return
    setBusy(true); setError('')
    try {
      await cancelMobilePairing()
      ++epoch.current; setReturnToWorkspace(connected || returnToWorkspace); setConnected(false); setLink(incoming.link); setIncoming(current => current?.kind === 'pair' && current.link === incoming.link ? null : current)
    } catch { setError(t('Не удалось подготовить новое подключение. Сохранённый доступ не изменён.')) }
    finally { setBusy(false) }
  }
  const incomingDialog = incoming && <Modal title={t(incoming.kind === 'pair' ? 'Открыта ссылка подключения' : 'Некорректная ссылка MPAC')} subtitle={t('Ссылка не меняет сохранённое подключение без вашего подтверждения.')} busy={busy} onClose={() => setIncoming(null)}>
    {incoming.kind === 'pair' ? <><p className="incoming-endpoint">{incoming.endpoint}</p><p>{t('Проверьте адрес компьютера. Затем подключитесь и сравните код на обоих устройствах.')}</p>{(busy || pending) && <p>{t('Сначала завершите или отмените текущее подключение.')}</p>}{error && <p role="alert">{error}</p>}<div className="modal-actions"><button className="button" disabled={busy} onClick={() => setIncoming(null)}>{t('Отмена')}</button><button className="button primary" disabled={loading || busy || !!pending} onClick={() => void reviewIncoming()}>{t('Проверить подключение')}</button></div></> : <><p>{t('Создайте новую ссылку подключения на компьютере.')}</p><div className="modal-actions"><button className="button" onClick={() => setIncoming(null)}>{t('Закрыть')}</button></div></>}
  </Modal>
  if (connected) return <><div inert={!!incoming}><App /></div>{incomingDialog}</>
  return <main className="phone-connect"><header><span className="brand-symbol">m<span>·</span></span><LanguageSwitcher /></header>
    <div className="phone-connect-body"><span className="eyebrow">MPAC MOBILE</span><h1>{t('Ваши агенты — под рукой.')}</h1><p>{t('Подключите телефон к MPAC на компьютере. Проекты, файлы и агенты остаются на компьютере.')}</p>
    {loading ? <p role="status">{t('Открываем подключение…')}</p> : pending ? <section className="phone-pair-confirm"><h2>{t('Подтвердите на компьютере')}</h2><strong className="phone-pair-code">{pending.code}</strong><p>{t('Сравните эти шесть цифр и разрешите подключение телефона на компьютере.')}</p><button className="button" onClick={() => void cancel()}>{t('Отмена')}</button></section> : <form onSubmit={e => { e.preventDefault(); void begin() }}>
      <label>{t('Название телефона')}<input value={name} onChange={e => setName(e.target.value)} maxLength={80} autoComplete="off" required disabled={busy} /></label>
      <label>{t('Ссылка привязки')}<textarea value={link} onChange={e => setLink(e.target.value)} maxLength={16384} placeholder="mpac://pair?…" autoCapitalize="none" autoCorrect="off" spellCheck={false} required disabled={busy} /></label>
      <p className="phone-connect-hint">{t('Создайте ссылку на компьютере и откройте её на этом устройстве или вставьте ниже.')}</p>
      <button className="button primary" disabled={busy || !link.trim() || !name.trim()}>{busy ? t('Подключаемся…') : t('Подключить телефон')}</button>
      {busy && <button type="button" className="button" onClick={() => void cancel()}>{t('Отмена')}</button>}
    </form>}
    {error && <p className="inline-error" role="alert">{error}</p>}
    {returnToWorkspace && !pending && <button className="button" disabled={busy} onClick={() => { setReturnToWorkspace(false); setConnected(true) }}>{t('Вернуться в рабочее пространство')}</button>}
    {!loading && <button className="button subtle" onClick={() => void forget()} disabled={busy}>{t('Забыть сохранённое подключение')}</button>}
    <p className="phone-connect-hint">{t('Первое подключение работает в одной локальной сети. Каждому телефону выдаётся отдельный доступ.')}</p>
    </div>{incomingDialog}</main>
}
