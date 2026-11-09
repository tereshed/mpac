import { t } from './i18n.ts'
import { useEffect, useRef } from 'react'
import type { ReactNode } from 'react'
import LanguageSwitcher from './LanguageSwitcher'

const modalStack: HTMLElement[] = []

export default function Modal({ title, subtitle, onClose, children, busy = false }: {
  title: string; subtitle: string; onClose: () => void; children: ReactNode; busy?: boolean
}) {
  const section = useRef<HTMLElement>(null)
  const controls = useRef({ onClose, busy })
  controls.current = { onClose, busy }
  useEffect(() => {
    const previous = document.activeElement
    const element = section.current
    if (element) modalStack.push(element)
    const focusable = () => [...(section.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea:not(:disabled),a[href],summary,[tabindex]:not([tabindex="-1"])') || [])].filter(element => element.getClientRects().length > 0)
    // Focusing the language select can open the system picker on iOS.
    const focusInitial = () => (focusable().find(element => !element.hasAttribute('data-language-selector')) || focusable()[0])?.focus({ preventScroll: true })
    if (!section.current?.contains(document.activeElement)) focusInitial()
    const keyboard = (event: KeyboardEvent) => {
      if (modalStack.at(-1) !== section.current) return
      if (event.key === 'Escape') {
        event.preventDefault(); event.stopPropagation()
        if (!controls.current.busy) controls.current.onClose()
      }
      if (event.key !== 'Tab') return
      const items = focusable()
      if (!items.length) { event.preventDefault(); return }
      const first = items[0], last = items[items.length - 1]
      if (event.shiftKey && (document.activeElement === first || !section.current?.contains(document.activeElement))) { event.preventDefault(); last.focus() }
      else if (!event.shiftKey && (document.activeElement === last || !section.current?.contains(document.activeElement))) { event.preventDefault(); first.focus() }
    }
    const keepFocus = (event: FocusEvent) => {
      if (modalStack.at(-1) === section.current && event.target instanceof Node && !section.current?.contains(event.target)) focusInitial()
    }
    document.addEventListener('keydown', keyboard)
    document.addEventListener('focusin', keepFocus)
    return () => {
      const index = element ? modalStack.indexOf(element) : -1
      if (index >= 0) modalStack.splice(index, 1)
      document.removeEventListener('keydown', keyboard)
      document.removeEventListener('focusin', keepFocus)
      const mobileTextEntry = window.matchMedia('(max-width: 740px)').matches && (previous instanceof HTMLTextAreaElement || previous instanceof HTMLInputElement || (previous instanceof HTMLElement && previous.isContentEditable))
      if (!mobileTextEntry && previous instanceof HTMLElement && previous.isConnected && !previous.closest('[inert]') && !section.current?.contains(previous)) previous.focus({ preventScroll: true })
    }
  }, [])
  return <div className="modal-backdrop" onMouseDown={event => { if (!busy && event.target === event.currentTarget) onClose() }}><section ref={section} className="modal" role="dialog" aria-modal="true" aria-label={title} aria-busy={busy}><div className="modal-title"><div><h2>{title}</h2><p>{subtitle}</p></div><div className="modal-language"><LanguageSwitcher /></div><button type="button" className="icon-button" onClick={onClose} disabled={busy} aria-label={t("Закрыть")}>×</button></div>{children}</section></div>
}
