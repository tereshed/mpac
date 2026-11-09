import { useEffect, useRef, useState } from 'react'
import Modal from './Modal'
import { t } from './i18n'
import { voiceMessage, voiceRequest } from './voice'
import type { VoiceProvider, VoiceSettings } from './voice'

export default function VoiceSettingsDialog({ token, onClose, onSaved }: {
  token: string
  onClose: () => void
  onSaved: (settings: VoiceSettings) => void
}) {
  const [settings, setSettings] = useState<VoiceSettings | null>(null)
  const [apiKey, setApiKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const pending = useRef(false)
  useEffect(() => {
    let active = true
    void voiceRequest<VoiceSettings>(token, 'settings').then(
      value => { if (active) setSettings(value) },
      failure => { if (active) setError(failure.message) },
    )
    return () => { active = false }
  }, [token])

  async function save(removeKey = false) {
    if (pending.current || !settings) return
    pending.current = true
    setBusy(true)
    setError('')
    try {
      const value = await voiceRequest<VoiceSettings>(token, 'settings', {
        provider: settings.provider,
        enabled: settings.enabled,
        base_url: settings.base_url.trim(),
        model: settings.model.trim(),
        api_key: apiKey.trim() || null,
        remove_key: removeKey,
      })
      setApiKey('')
      onSaved(value)
      onClose()
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : voiceMessage('voice_storage'))
    } finally {
      pending.current = false
      setBusy(false)
    }
  }

  function chooseProvider(provider: VoiceProvider) {
    const profile = settings?.providers.find(item => item.id === provider)
    if (!settings || !profile) return
    setSettings({ ...settings, provider, base_url: profile.base_url, model: profile.model, has_key: profile.has_key })
    setApiKey('')
    setError('')
  }

  const providerNames: Record<VoiceProvider, string> = {
    openai: 'OpenAI',
    openrouter: 'OpenRouter',
    groq: 'Groq',
    custom: t('Свой провайдер'),
  }

  return <Modal title={t('Голосовой ввод')} subtitle={t('Запись отправляется в выбранный API после нажатия «Распознать». Ключ хранится на этом устройстве.')} busy={busy} onClose={onClose}>
    {settings ? <form className="modal-form voice-settings" onSubmit={event => { event.preventDefault(); void save() }}>
      <label className="voice-toggle"><input type="checkbox" checked={settings.enabled} disabled={busy} onChange={event => setSettings({ ...settings, enabled: event.target.checked })} />{t('Включить голосовой ввод')}</label>
      <label>{t('Провайдер распознавания')}<select value={settings.provider} disabled={busy} onChange={event => chooseProvider(event.target.value as VoiceProvider)}>
        {settings.providers.map(profile => <option key={profile.id} value={profile.id}>{providerNames[profile.id]}</option>)}
      </select></label>
      <label>{t('Базовый URL API')}<input type="url" required value={settings.base_url} readOnly={settings.provider !== 'custom'} disabled={busy} autoCapitalize="off" autoCorrect="off" spellCheck={false} maxLength={2048} onChange={event => setSettings({ ...settings, base_url: event.target.value })} /></label>
      <label>{t('Модель распознавания')}<input required value={settings.model} disabled={busy} autoCapitalize="off" autoCorrect="off" spellCheck={false} maxLength={128} onChange={event => setSettings({ ...settings, model: event.target.value })} /></label>
      <label>{t('Личный API-ключ')}<input type="password" autoComplete="new-password" value={apiKey} disabled={busy} autoCapitalize="off" autoCorrect="off" spellCheck={false} maxLength={4096} placeholder={settings.has_key ? t('Ключ сохранён') : ''} onChange={event => setApiKey(event.target.value)} /></label>
      <p className="form-help">{settings.provider === 'custom'
        ? t('Нужен API, совместимый с OpenAI Audio Transcriptions. Можно указать другого провайдера и его модель.')
        : t('У каждого провайдера свой ключ и модель. Они сохраняются при переключении.')}</p>
      {error && <p className="inline-error" role="alert">{error}</p>}
      <div className="modal-actions">
        {settings.has_key && <button type="button" className="button secondary" disabled={busy} onClick={() => void save(true)}>{t('Удалить ключ')}</button>}
        <button type="button" className="button secondary" disabled={busy} onClick={onClose}>{t('Отмена')}</button>
        <button type="submit" className="button primary" disabled={busy}>{t('Сохранить')}</button>
      </div>
    </form> : error ? <p className="inline-error" role="alert">{error}</p> : <p role="status">{t('Загрузка…')}</p>}
  </Modal>
}
