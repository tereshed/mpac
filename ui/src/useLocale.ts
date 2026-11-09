import { useSyncExternalStore } from 'react'
import { getLocale, subscribeLocale } from './i18n.ts'

export default function useLocale() { return useSyncExternalStore(subscribeLocale, getLocale, () => 'en' as const) }
