import voice from './locales/voice.ts'
import shell from './locales/shell.ts'
import touchTerminal from './locales/touch-terminal.ts'
import mobile from './locales/mobile.ts'
import phone from './locales/phone.ts'
import common from './locales/common.ts'
import environment from './locales/environment.ts'
import projects from './locales/projects.ts'
import review from './locales/review.ts'
import terminal from './locales/terminal.ts'
import errorsProjects from './locales/errors-projects.ts'
import errorsReview from './locales/errors-review.ts'
import errorsTerminal from './locales/errors-terminal.ts'
import activityDetails from './locales/activity-details.ts'
import type { Locale, Messages, MessageValues } from './i18n-types.ts'

export type { Locale, Messages, MessageValues } from './i18n-types.ts'
export const LOCALE_KEY = 'mpac-locale'
export const supportedLocales = ['en', 'ru', 'es'] as const
export const messages: Messages = { ...errorsProjects, ...errorsReview, ...errorsTerminal, ...projects, ...review, ...terminal, ...common, ...activityDetails, ...environment, ...mobile, ...phone, ...touchTerminal, ...shell, ...voice }
export const isLocale = (value: unknown): value is Locale => supportedLocales.some(locale => locale === value)
export function storedLocale(): Locale {
  try { const value = globalThis.localStorage?.getItem(LOCALE_KEY); return isLocale(value) ? value : 'en' }
  catch { return 'en' }
}
let currentLocale: Locale = storedLocale()
const listeners = new Set<() => void>()
const aliases = new Map<string, string>()
const translated = new Map<string, string>()
const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
const templates: { key: string; names: string[]; pattern: RegExp; specificity: number }[] = []
for (const [key, entry] of Object.entries(messages)) {
  for (const text of [key, entry.en, entry.es]) {
    aliases.set(text, key)
    const names: string[] = [], chunks: string[] = []
    let start = 0
    for (const match of text.matchAll(/\{(\w+)\}/g)) {
      chunks.push(escaped(text.slice(start, match.index)), '([\\s\\S]*?)')
      names.push(match[1]); start = match.index + match[0].length
    }
    if (names.length) { chunks.push(escaped(text.slice(start))); templates.push({ key, names, pattern: new RegExp('^' + chunks.join('') + '$', 'u'), specificity: text.replace(/\{\w+\}/g, '').length }) }
  }
}
templates.sort((a, b) => b.specificity - a.specificity)
export const getLocale = (): Locale => currentLocale
export const localeTag = (): string => ({ en: 'en-US', ru: 'ru-RU', es: 'es-ES' })[currentLocale]
export function subscribeLocale(listener: () => void): () => void { listeners.add(listener); return () => { listeners.delete(listener) } }
function format(source: string, values?: MessageValues): string {
  return source.replace(/\{(\w+)\}/g, (placeholder, name: string) => values && Object.hasOwn(values, name) ? String(values[name]) : placeholder)
}
/** Only explicit application messages cross this boundary. User names, paths,
 * Markdown, diffs and CLI output remain their original content. */
export function t(source: string, values?: MessageValues): string {
  const key = Object.hasOwn(messages, source) ? source : aliases.get(source)
  if (key) return format(currentLocale === 'ru' ? key : messages[key][currentLocale], values)
  if (values || source.length > 4096) return format(source, values)
  const cached = translated.get(source)
  if (cached !== undefined) return cached
  // Notices/errors may have been produced before a language change. Match only
  // known complete message templates, preserving every interpolated value.
  for (const template of templates) {
    const match = template.pattern.exec(source)
    if (!match) continue
    const parameters = Object.fromEntries(template.names.map((name, index) => [name, match[index + 1]]))
    const result = format(currentLocale === 'ru' ? template.key : messages[template.key][currentLocale], parameters)
    if (translated.size >= 512) translated.clear()
    translated.set(source, result)
    return result
  }
  if (translated.size >= 512) translated.clear()
  translated.set(source, source)
  return source
}
export function localizedCount(value: number, forms: [string, string, string]): string {
  const plural = new Intl.PluralRules(localeTag()).select(value)
  return `${new Intl.NumberFormat(localeTag()).format(value)} ${t(forms[plural === 'one' ? 0 : plural === 'few' ? 1 : 2])}`
}
function applyDocumentLocale() {
  if (typeof document === 'undefined') return
  document.documentElement.lang = currentLocale
  document.title = t('MPAC · Рабочее пространство')
}
export function setLocale(locale: Locale, persist = true) {
  if (!isLocale(locale)) return
  if (persist) { try { globalThis.localStorage?.setItem(LOCALE_KEY, locale) } catch { /* In-memory selection still works. */ } }
  if (currentLocale === locale) { applyDocumentLocale(); return }
  currentLocale = locale; translated.clear(); applyDocumentLocale()
  for (const listener of listeners) listener()
}
applyDocumentLocale()
if (typeof window !== 'undefined') window.addEventListener('storage', event => {
  if (event.key === LOCALE_KEY || event.key === null) setLocale(isLocale(event.newValue) ? event.newValue : 'en', false)
})
