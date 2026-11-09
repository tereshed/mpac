import { localizedCount, t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage, query } from './api'
import type { Project, Root, Session } from './api'
import type { LaunchForReview, StructuredReview } from './review-types'
import LegacyReviewPane from './LegacyReviewPane'
import CommentableCode from './CommentableCode'
import ReviewCommentsPanel from './ReviewCommentsPanel'
import { RootSelect } from './FilesPane'
import './review-comments.css'

interface Props {
  token: string; project: Project; root: Root; session?: Session; onRoot: (id: string) => void; onSent: () => void;
  externalToolbar?: boolean; repositoryRevision?: number; repositoryBusy?: boolean; commentsAvailable?: boolean; sessions?: Session[];
  onLaunchForReview?: LaunchForReview; onOpenSession?: (session: Session) => void;
}
export default function ReviewPane(props: Props) {
  return props.commentsAvailable ? <StructuredReviewPane key={props.token + '\0' + props.project.id + '\0' + props.root.id} {...props} /> : <LegacyReviewPane {...props} />
}
function StructuredReviewPane({ token, project, root, externalToolbar = false, repositoryRevision = 0, repositoryBusy = false, sessions = [], onRoot, onLaunchForReview, onOpenSession }: Props) {
  const [review, setReview] = useState<StructuredReview | null>(null)
  const [selectedFile, setSelectedFile] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [refresh, setRefresh] = useState(0)
  const [commentsRevision, setCommentsRevision] = useState(0)
  const [showComments, setShowComments] = useState(true)
  const generation = useRef(0)
  useEffect(() => {
    const expected = ++generation.current
    if (repositoryBusy) { setReview(null); setLoading(true); return }
    const abort = new AbortController(); setLoading(true); setError('')
    api<StructuredReview>(token, `/projects/${project.id}/review-lines?${query({ root_id: root.id })}`, undefined, abort.signal).then(result => {
      if (abort.signal.aborted || generation.current !== expected) return
      setReview(result); setSelectedFile(previous => previous && result.blocks.some(block => block.path === previous) ? previous : null)
    }).catch(failure => { if (!abort.signal.aborted && generation.current === expected) { setReview(null); setError(errorMessage(failure)) } }).finally(() => { if (!abort.signal.aborted && generation.current === expected) setLoading(false) })
    return () => abort.abort()
  }, [token, project.id, root.id, refresh, repositoryRevision, repositoryBusy])
  const paths = [...new Set(review?.blocks.map(block => block.path) || [])]
  const blocks = review?.blocks.filter(block => !selectedFile || block.path === selectedFile) || []
  return <div className="review-layout structured-review">
    {!externalToolbar && <div className="panel-toolbar"><RootSelect project={project} root={root} onChange={onRoot} /><span className="path-hint">{root.path}</span></div>}
    <div className="review-summary"><span className="eyebrow">{t("РАБОЧИЕ ИЗМЕНЕНИЯ")}</span><span>{review ? localizedCount(paths.length, ['файл', 'файла', 'файлов']) : '…'}</span><span className="mono muted">{review?.head?.slice(0, 10)}</span><button type="button" className="button secondary compact" disabled={loading || repositoryBusy} onClick={() => setRefresh(value => value + 1)}>{t("Обновить")}</button><button type="button" className="button secondary compact" aria-pressed={showComments} onClick={() => setShowComments(value => !value)}>{showComments ? t("Скрыть комментарии") : t("Комментарии")}</button></div>
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    {review?.truncated && <div className="notice">{t("Снимок изменений ограничен по размеру. Сохранённые комментарии доступны, но новые привязки и передача требуют полного снимка.")}</div>}
    <div className={`review-workspace ${showComments ? 'with-comments' : ''}`}>
      <div className="review-code-workspace">
        {!!paths.length && <nav className="review-file-list" aria-label={t("Изменённые файлы")}><button type="button" className={`review-file-button ${!selectedFile ? 'selected' : ''}`} onClick={() => setSelectedFile(null)}>{t("Все изменения ·")}{' '}{paths.length}</button>{paths.map(path => <button type="button" key={path} className={`review-file-button ${selectedFile === path ? 'selected' : ''}`} onClick={() => setSelectedFile(path)}><span className="review-file-path">{path}</span></button>)}</nav>}
        <div className="review-structured-blocks">{loading ? <div className="empty-state"><p>{repositoryBusy ? t("Обновляем рабочую папку…") : t("Собираем изменения…")}</p></div> : blocks.length ? blocks.map(block => <section className="review-structured-block" key={block.path + '\0' + block.id}><div className="review-diff-title"><span>{block.path}</span><span className="review-diff-status">{block.section === 'staged' ? t("Подготовлено к коммиту") : block.section === 'untracked' ? t("Новый файл") : t("Рабочие изменения")}</span></div>{(block.lines_truncated || block.lines.length > 2000) && <div className="notice">{t("Показана часть строк этого блока либо diff распознан не полностью. Для ограниченного снимка комментирование отключено; можно открыть исходник во вкладке «Файлы».")}</div>}{!block.lines.length && <pre className="commit-diff-preview">{block.raw || t("Изменение без текстовых строк: бинарный файл, режим доступа или переименование.")}</pre>}<CommentableCode token={token} projectId={project.id} rootId={root.id} path={block.path} source="diff" snapshot={review!.snapshot} blockId={block.id} lines={block.lines.slice(0, 2000)} disabled={repositoryBusy || review!.truncated || block.lines_truncated} onCreated={() => { setCommentsRevision(value => value + 1); setShowComments(true) }} /></section>) : <div className="empty-state"><div className="empty-symbol">✓</div><h3>{error ? t("Ревью недоступно") : t("Рабочая папка чистая")}</h3><p>{error ? t("Повторите запрос или выберите другую папку.") : t("Комментарии остаются доступны справа, даже когда незакоммиченных изменений нет.")}</p></div>}</div>
      </div>
      {showComments && <ReviewCommentsPanel token={token} project={project} root={root} sessions={sessions} revision={commentsRevision + repositoryRevision} onLaunchForReview={onLaunchForReview} onOpenSession={onOpenSession} />}
    </div>
  </div>
}
