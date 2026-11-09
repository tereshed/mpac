import { useEffect, useRef, useState } from 'react'
import { mobileClient } from './native-mobile'

const mobileQuery = '(max-width: 740px)'
const textEntry = () => {
  const element = document.activeElement
  return element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement || (element instanceof HTMLElement && element.isContentEditable)
}

export default function useMobileWorkspace() {
  const [mobile, setMobile] = useState(() => window.matchMedia(mobileQuery).matches)
  const baseline = useRef(window.innerHeight)
  const layoutWidth = useRef(window.innerWidth)
  const [viewport, setViewport] = useState({ height: window.visualViewport?.height || window.innerHeight, top: 0, bottom: 0, keyboardOpen: false })
  useEffect(() => {
    const media = window.matchMedia(mobileQuery)
    const change = () => setMobile(media.matches)
    media.addEventListener('change', change)
    return () => media.removeEventListener('change', change)
  }, [])
  useEffect(() => {
    // Native tablets also resize for their keyboard and windowed multitasking.
    if (!mobile && !mobileClient()) return
    const visual = window.visualViewport
    let frame = 0
    const measure = () => {
      const height = Math.round(visual?.height || window.innerHeight)
      const top = Math.max(0, Math.round(visual?.offsetTop || 0))
      const editing = textEntry()
      if (!editing || layoutWidth.current !== window.innerWidth) baseline.current = window.innerHeight
      layoutWidth.current = window.innerWidth
      setViewport({ height, top, bottom: Math.max(0, window.innerHeight - height - top), keyboardOpen: editing && baseline.current - height > 100 })
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure) }
    measure()
    window.addEventListener('resize', schedule)
    visual?.addEventListener('resize', schedule)
    visual?.addEventListener('scroll', schedule)
    document.addEventListener('focusin', schedule)
    document.addEventListener('focusout', schedule)
    return () => {
      cancelAnimationFrame(frame)
      window.removeEventListener('resize', schedule)
      visual?.removeEventListener('resize', schedule)
      visual?.removeEventListener('scroll', schedule)
      document.removeEventListener('focusin', schedule)
      document.removeEventListener('focusout', schedule)
    }
  }, [mobile])
  return { mobile, ...viewport }
}
