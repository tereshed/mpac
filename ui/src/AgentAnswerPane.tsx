import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from './api'
import { activityTime } from './activity'
import { t } from './i18n'

interface Answer { text: string; saved_at: number }

export default function AgentAnswerPane({ token, sessionId, onClose }: { token: string; sessionId: string; onClose: () => void }) {
  const [answer, setAnswer] = useState<Answer | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [reload, setReload] = useState(0)
  const view = useRef<HTMLPreElement>(null)

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

  return <section className="terminal-history terminal-answer" aria-label={t('Последний ответ')}>
    <div className="terminal-answer-header">
      <div><strong>{t('Последний ответ')}</strong>{answer && <time>{activityTime(answer.saved_at)}</time>}</div>
      <button type="button" onClick={() => setReload(value => value + 1)} disabled={loading}>{t('Обновить')}</button>
      <button type="button" onClick={onClose}>{t('К терминалу')}</button>
    </div>
    {error && <p role="alert">{error}</p>}
    {answer ? <pre ref={view} tabIndex={0}>{answer.text}</pre>
      : !error && <p role="status">{loading ? t('Загружаем ответ…') : t('Пока нет сохранённого ответа. Если агент уже ответил, проверьте вывод терминала.')}</p>}
  </section>
}
