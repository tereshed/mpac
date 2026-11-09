import { useId } from 'react'
import { isLocale, setLocale, t } from './i18n.ts'
import useLocale from './useLocale'
import './language-switcher.css'

export default function LanguageSwitcher() {
  const locale = useLocale(), id = useId()
  return <label className="language-switcher" htmlFor={id}><span aria-hidden="true">◎</span><span className="sr-only">{t('Язык')}</span><select id={id} value={locale} aria-label={t('Язык')} data-language-selector onChange={event => { if (isLocale(event.target.value)) setLocale(event.target.value) }}>
    <option value="en">English</option><option value="ru">Русский</option><option value="es">Español</option>
  </select></label>
}
