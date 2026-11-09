export type Locale = 'en' | 'ru' | 'es'
export type Messages = Record<string, { en: string; es: string }>
export type MessageValues = Record<string, string | number | boolean | null | undefined>
