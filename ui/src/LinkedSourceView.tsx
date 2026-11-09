import { t } from './i18n.ts'
import { useEffect, useRef } from 'react'
import { sourceLineWindow } from './file-navigation'

/** Fallback for old daemons or lines beyond the structured source limit. */
export default function LinkedSourceView({ content, line, endLine }: { content: string; line?: number; endLine?: number }) {
  const element = useRef<HTMLDivElement>(null)
  const lines = content ? content.split('\n').map(text => text.endsWith('\r') ? text.slice(0, -1) : text) : []
  if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop()
  const window = sourceLineWindow(lines.length, line)
  useEffect(() => {
    if (line && window.found) element.current?.querySelector<HTMLElement>(`[data-source-line="${line}"]`)?.scrollIntoView({ block: 'center' })
  }, [line, content, window.found])
  return <div className="linked-source-view" ref={element}>
    {!window.found && <p className="notice">{t("Строки {p0} нет в текущем файле. Показано начало файла.", { p0: line })}</p>}
    {(window.start > 0 || window.end < lines.length) && <p className="source-review-notice">{t("Показаны строки {p0}–{p1} из {p2}.", { p0: window.start + 1, p1: window.end, p2: lines.length })}</p>}
    {lines.slice(window.start, window.end).map((text, index) => {
      const number = window.start + index + 1
      return <div key={number} className={`linked-source-row ${line && number >= line && number <= (endLine || line) ? 'file-linked-line' : ''}`} data-source-line={number}><span>{number}</span><code>{text || ' '}</code></div>
    })}
  </div>
}
