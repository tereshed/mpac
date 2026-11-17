import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

interface Action { label: string; run: () => void; destructive?: boolean }

export default function RailActionsMenu({ id, label, title, triggerLabel, actions }: {
  id: string; label: string; title: string; triggerLabel: string; actions: Action[]
}) {
  const [open, setOpen] = useState(false)
  const [position, setPosition] = useState({ left: 8, top: 8 })
  const trigger = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const startAtEnd = useRef(false)
  const contains = (target: Node | null) => target === trigger.current || menu.current?.contains(target)
  const reveal = (atEnd = false) => {
    const rect = trigger.current?.getBoundingClientRect()
    if (!rect) return
    const menuHeight = actions.length * 44 + 12
    setPosition({ left: Math.max(8, Math.min(window.innerWidth - 218, rect.right - 210)), top: rect.bottom + menuHeight < window.innerHeight ? rect.bottom + 5 : Math.max(8, rect.top - menuHeight) })
    startAtEnd.current = atEnd
    setOpen(true)
  }
  useEffect(() => {
    if (!open) return
    const items = () => [...(menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') || [])]
    const initial = items()
    initial[startAtEnd.current ? initial.length - 1 : 0]?.focus()
    const outside = (event: PointerEvent) => { if (!contains(event.target as Node)) setOpen(false) }
    const closeOnLayout = (event: Event) => { if (!menu.current?.contains(event.target as Node)) setOpen(false) }
    const keyboard = (event: KeyboardEvent) => {
      if (!contains(document.activeElement)) return
      if (event.key === 'Escape') { event.preventDefault(); setOpen(false); trigger.current?.focus(); return }
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return
      event.preventDefault()
      const list = items()
      const current = list.indexOf(document.activeElement as HTMLButtonElement)
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? list.length - 1 : (current + (event.key === 'ArrowDown' ? 1 : -1) + list.length) % list.length
      list[next]?.focus()
    }
    document.addEventListener('pointerdown', outside)
    document.addEventListener('keydown', keyboard)
    document.addEventListener('scroll', closeOnLayout, true)
    window.addEventListener('resize', closeOnLayout)
    return () => {
      document.removeEventListener('pointerdown', outside)
      document.removeEventListener('keydown', keyboard)
      document.removeEventListener('scroll', closeOnLayout, true)
      window.removeEventListener('resize', closeOnLayout)
    }
  }, [open])
  const blur = (next: EventTarget | null) => { if (!contains(next as Node | null)) setOpen(false) }
  return <>
    <button type="button" className="project-actions-trigger" ref={trigger} aria-label={triggerLabel} title={title} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined} onBlur={event => blur(event.relatedTarget)} onClick={() => { if (open) setOpen(false); else reveal() }} onKeyDown={event => {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault(); event.stopPropagation(); reveal(event.key === 'ArrowUp')
      }
    }}>⋯</button>
    {open && createPortal(<div ref={menu} className="project-actions-menu" style={position} role="menu" id={id} aria-label={label} onBlur={event => blur(event.relatedTarget)}>
      {actions.map(action => <button key={action.label} type="button" role="menuitem" tabIndex={-1} className={action.destructive ? 'project-delete-action' : undefined} onClick={() => { setOpen(false); trigger.current?.focus(); action.run() }}>{action.label}</button>)}
    </div>, document.body)}
  </>
}
