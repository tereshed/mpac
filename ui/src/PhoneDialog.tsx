import { useEffect, useState } from 'react'
import { api, errorMessage } from './api'
import { t } from './i18n'
import Modal from './Modal'
import './phone-dialog.css'

interface PhoneState { running: boolean; ip: string | null; share_page: string | null; interfaces: string[]; background?: boolean }
const isTailnet = (address: string) => { const [a,b] = address.split('.').map(Number); return a === 100 && b >= 64 && b <= 127 }
export const desktopPhoneAvailable = () => location.protocol === 'mpac:' && location.hostname === 'localhost'
export default function PhoneDialog({ token, onClose }: { token: string; onClose: () => void }) {
  const [state, setState] = useState<PhoneState | null>(null)
  const [ip, setIp] = useState('')
  const [page, setPage] = useState('https://mpac.works/open/')
  const [background, setBackground] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [opened, setOpened] = useState(false)
  useEffect(() => {
    const controller = new AbortController()
    api<PhoneState>(token, '/native/phone/state', undefined, controller.signal).then(value => {
      setState(value); const address = value.ip || value.interfaces[0] || ''; setIp(address); setBackground(value.background || isTailnet(address)); if (value.share_page) setPage(value.share_page)
    }).catch(e => { if (!controller.signal.aborted) setError(errorMessage(e)) })
    return () => controller.abort()
  }, [token])
  const open = async () => {
    if (busy) return
    setBusy(true); setError(''); setOpened(false)
    try { await api(token, '/native/phone/open', { ip, share_page: page, background }); setOpened(true); setState(previous => previous ? { ...previous, running: true, ip, share_page: page, background } : previous) }
    catch (e) { setError(errorMessage(e)) } finally { setBusy(false) }
  }
  const stop = async () => {
    if (busy) return
    setBusy(true); setError('')
    try { await api(token, '/native/phone/stop', {}); setState(previous => previous ? { ...previous, running: false } : previous); setOpened(false) }
    catch (e) { setError(errorMessage(e)) } finally { setBusy(false) }
  }
  return <Modal title={t('Телефон и Telegram')} subtitle={t('Подключите личный Telegram-бот и отправляйте себе ссылки на MPAC.')} onClose={onClose} busy={busy}>
    <p>{t('Выберите сеть компьютера. Телефону нужен доступ к ней; Telegram доставляет ссылки и не заменяет сетевое подключение.')}</p>
    <label>{t('Адрес подключения')}<select value={ip} onChange={e => { setIp(e.target.value); setBackground(isTailnet(e.target.value)) }} disabled={busy || !!state?.running}>{state?.interfaces.map(address => <option key={address} value={address}>{address}{isTailnet(address) ? ' · Tailscale' : ' · LAN'}</option>)}</select></label>
    {state && !state.interfaces.length && <p role="status">{t('Не найден адрес локальной сети. Подключите компьютер к Wi-Fi или Ethernet.')}</p>}
    {isTailnet(ip) && <p className="muted">{t('Tailscale: работает через Wi-Fi и мобильный интернет. Tailscale должен быть включён на обоих устройствах в одном аккаунте.')}</p>}
    <label className="phone-background-toggle"><input type="checkbox" checked={background} onChange={e => setBackground(e.target.checked)} disabled={busy || !!state?.running} />{t('Сохранять мобильный доступ после выхода из MPAC')}</label>
    <label>{t('Публичная страница открытия')}<input type="url" value={page} onChange={e => setPage(e.target.value)} disabled={busy || !!state?.running} /></label>
    <p className="muted">{t('Откроется отдельное окно: привязка телефона, токен BotFather, подтверждение личного чата и отправка ссылок. Сессии агентов продолжат работу.')}</p>
    <p className="muted">{background ? t('Мобильный доступ работает в фоне и запускается при входе в macOS. Чтобы отключить его, нажмите «Отключить мобильный доступ». Mac должен оставаться включённым и доступным по сети.') : t('Закрытие окна сохраняет подключение. Выход из MPAC отключает мобильный доступ до следующего запуска подключения.')}</p>
    {state?.running && <p role="status">{t('Подключение активно')}: {ip}{state.background ? ' · ' + t('Фоновая служба') : ''}</p>}
    {error && <div className="inline-error" role="alert">{error}</div>}
    {opened && <p role="status">{t('Окно подключения открыто.')}</p>}
    <div className="modal-actions">{state?.running && <button type="button" className="button" onClick={stop} disabled={busy}>{t('Отключить мобильный доступ')}</button>}<button type="button" className="button primary" onClick={open} disabled={busy || !ip || !state}>{busy ? t('Открываем…') : t('Открыть подключение')}</button></div>
  </Modal>
}
