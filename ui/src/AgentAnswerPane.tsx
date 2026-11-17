import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from './api'
import { activityTime } from './activity'
import { t } from './i18n'

interface Answer { text: string; saved_at: number; revision?: number | null }

export default function AgentAnswerPane({ token, sessionId, visible, latestRevision, readRevision, onMarkViewed, onClose }: {
  token: string; sessionId: string; visible: boolean; latestRevision: number; readRevision: number;
  onMarkViewed?: (revision: number) => Promise<void>; onClose: () => void;
}) {
  const [answer, setAnswer] = useState<Answer | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [reload, setReload] = useState(0)
  const view = useRef<HTMLPreElement>(null)
  const markViewed = useRef(onMarkViewed); markViewed.current = onMarkViewed
  const attempted = useRef<number | null>(null)

  useEffect(() => {
    const abort = new AbortController()
    setLoading(true); setError('')
    void api<{ answer: Answer | null }>(token, `/sessions/${encodeURIComponent(sessionId)}/answer`, undefined, abort.signal)
      .then(value => { if (!abort.signal.aborted) setAnswer(value.answer) })
      .catch(failure => { if (!abort.signal.aborted) setError(errorMessage(failure)) })
      .finally(() => { if (!abort.signal.aborted) setLoading(false) })
    return () => abort.abort()
  }, [token, sessionId, reload])
  useEffect(() => { if (view.current) view.current.scrollTop = 0 }, [answer])
  const revision = answer?.revision
  useEffect(() => {
    if (!visible || loading || error || typeof revision !== 'number' || !Number.isSafeInteger(revision)
      || revision <= readRevision || revision < latestRevision) return
    const acknowledge = () => {
      if (document.visibilityState !== 'visible' || attempted.current === revision || !markViewed.current) return
      attempted.current = revision
      // The server compares this exact revision with the current result. A
      // concurrent new response must keep its unread badge.
      void markViewed.current(revision).catch(() => { /* Keep the unread badge and allow a refresh or manual acknowledgement. */ })
    }
    const frame = requestAnimationFrame(acknowledge)
    document.addEventListener('visibilitychange', acknowledge)
    return () => { cancelAnimationFrame(frame); document.removeEventListener('visibilitychange', acknowledge) }
  }, [visible, loading, error, revision, readRevision, latestRevision, reload])

  return <section className="terminal-history terminal-answer" aria-label={t('Последний ответ')}>
    <div className="terminal-answer-header">
      <div><strong>{t('Последний ответ')}</strong>{answer && <span className="terminal-answer-meta"><time>{activityTime(answer.saved_at)}</time>{typeof revision === 'number' && revision > 0 && revision <= readRevision && <span>{t('Просмотрено')}</span>}</span>}</div>
      <button type="button" onClick={() => { attempted.current = null; setReload(value => value + 1) }} disabled={loading}>{t('Обновить')}</button>
      <button type="button" onClick={onClose}>{t('К терминалу')}</button>
    </div>
    {error && <p role="alert">{error}</p>}
    {answer ? <pre ref={view} tabIndex={0}>{answer.text}</pre>
      : !error && <p role="status">{loading ? t('Загружаем ответ…') : t('Пока нет сохранённого ответа. Если агент уже ответил, проверьте вывод терминала.')}</p>}
  </section>
}
