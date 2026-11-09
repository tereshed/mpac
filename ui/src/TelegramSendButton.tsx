import { useMemo, useRef, useState } from 'react'
import { errorMessage } from './api'
import { t } from './i18n'
import { telegramLinkSender } from './telegram-shortcut'

export default function TelegramSendButton({ token, onConfigure }: {token: string; onConfigure: () => void}) {
  const send = useMemo(() => telegramLinkSender(token), [token])
  const pending = useRef(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [sent, setSent] = useState(false)
  const submit = async () => {
    if (pending.current) return
    pending.current = true; setBusy(true); setError(''); setSent(false)
    try { if (await send()) setSent(true) }
    catch (e) { setError(errorMessage(e)) }
    finally { pending.current = false; setBusy(false) }
  }
  return <div className="telegram-shortcut">
    <button className="archived-projects-trigger" onClick={submit} disabled={busy} aria-busy={busy}>{busy ? t('Отправляем ссылку…') : t('Отправить ссылку в Telegram')}</button>
    <p>{t('Ссылка подключит телефон к этому компьютеру. Первое подключение нужно подтвердить здесь.')}</p>
    {sent && <p role="status">{t('Приглашение к этому компьютеру отправлено. Откройте его в течение 3 минут.')}</p>}
    {error && <div role="alert"><p>{t(error)}</p><button type="button" onClick={onConfigure}>{t('Настроить Telegram')}</button></div>}
  </div>
}
