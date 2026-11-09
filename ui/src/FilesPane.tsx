import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { api, errorMessage, query } from './api'
import type { FileEntry, Project, Root, Session } from './api'
import BranchSelect from './BranchSelect'
import type { LaunchForReview, SourceLines } from './review-types'
import CommentableCode from './CommentableCode'
import ReviewCommentsPanel from './ReviewCommentsPanel'
import type { FileOpenRequest } from './file-navigation'
import { sourceLineWindow } from './file-navigation'
import LinkedSourceView from './LinkedSourceView'
import './terminal-file-links.css'
import './review-comments.css'

function relativeTarget(current: string, href: string): string | null {
  if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('//') || href.startsWith('/') || href.startsWith('#')) return null
  const segments = current.split('/').slice(0, -1)
  for (const part of decodeURIComponent(href.split(/[?#]/)[0]).split('/')) {
    if (!part || part === '.') continue
    if (part === '..') { if (!segments.length) return null; segments.pop() }
    else segments.push(part)
  }
  return segments.join('/')
}

export function RootSelect({ project, root, onChange, disabled = false }: { project: Project; root: Root; onChange: (id: string) => void; disabled?: boolean }) {
  return <label className="root-select"><span>{t("Папка")}</span><select disabled={disabled} value={root.id} onChange={e => onChange(e.target.value)} aria-label={t("Папка проекта")}>{project.roots.map(item => <option key={item.id} value={item.id}>{item.name}</option>)}</select></label>
}

export default function FilesPane({ token, project, root, onRoot, branchesAvailable = false, externalToolbar = false, repositoryRevision = 0, repositoryBusy = false, commentsAvailable = false, sessions = [], onLaunchForReview, onOpenSession, requestedFile, onFileOpened }: { token: string; project: Project; root: Root; onRoot: (id: string) => void; branchesAvailable?: boolean; externalToolbar?: boolean; repositoryRevision?: number; repositoryBusy?: boolean; commentsAvailable?: boolean; sessions?: Session[]; onLaunchForReview?: LaunchForReview; onOpenSession?: (session: Session) => void; requestedFile?: FileOpenRequest | null; onFileOpened?: (requestId: string) => void }) {
  const [directory, setDirectory] = useState('')
  const [files, setFiles] = useState<FileEntry[]>([])
  const [selected, setSelected] = useState('')
  const [content, setContent] = useState<string | null>(null)
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(false)
  const [raw, setRaw] = useState(false)
  const [revision, setRevision] = useState(0)
  const [branchSwitching, setBranchSwitching] = useState(false)
  const [sourceLines, setSourceLines] = useState<SourceLines | null>(null)
  const [showComments, setShowComments] = useState(false)
  const [commentsRevision, setCommentsRevision] = useState(0)
  const [linkedLocation, setLinkedLocation] = useState<FileOpenRequest | null>(null)
  const readGeneration = useRef(0)
  const listingRequest = useRef<AbortController | null>(null)
  const documentRequest = useRef<AbortController | null>(null)
  const context = token + '\0' + project.id + '\0' + root.id
  const latestContext = useRef(context)
  if (latestContext.current !== context) { latestContext.current = context; readGeneration.current += 1 }

  const invalidateReads = () => {
    readGeneration.current += 1
    listingRequest.current?.abort(); documentRequest.current?.abort()
  }
  const resetView = () => {
    setDirectory(''); setSelected(''); setContent(null); setSourceLines(null); setFiles([]); setError(''); setRaw(false); setLinkedLocation(null)
  }
  useEffect(() => {
    invalidateReads(); resetView(); setLoading(true); setBranchSwitching(false)
  }, [token, root.id, project.id])
  useEffect(() => {
    if (!externalToolbar) return
    invalidateReads(); setContent(null); setSourceLines(null); setFiles([]); setLoading(true)
  }, [repositoryRevision, repositoryBusy, externalToolbar])
  useEffect(() => {
    if (!branchesAvailable && branchSwitching) { invalidateReads(); resetView(); setLoading(true); setBranchSwitching(false); setRevision(value => value + 1) }
  }, [branchesAvailable, branchSwitching])

  useEffect(() => {
    if (!requestedFile || requestedFile.projectId !== project.id || requestedFile.rootId !== root.id || branchSwitching || repositoryBusy) return
    invalidateReads(); setError(''); setContent(null); setSourceLines(null)
    setDirectory(requestedFile.path.split('/').slice(0, -1).join('/'))
    setSelected(requestedFile.path); setRaw(!!requestedFile.line); setLinkedLocation(requestedFile)
    setRevision(value => value + 1)
    onFileOpened?.(requestedFile.requestId)
  }, [requestedFile?.requestId, token, project.id, root.id, repositoryBusy, branchSwitching])

  useEffect(() => {
    if (branchSwitching || repositoryBusy) return
    const abort = new AbortController()
    const generation = readGeneration.current
    const current = () => !abort.signal.aborted && latestContext.current === context && generation === readGeneration.current
    listingRequest.current = abort
    setLoading(true)
    api<{ files: FileEntry[] }>(token, `/projects/${project.id}/files?${query({ root_id: root.id, path: directory })}`, undefined, abort.signal)
      .then(result => {
        if (!current()) return
        setFiles([...result.files].sort((a, b) => Number(b.is_dir) - Number(a.is_dir) || a.path.localeCompare(b.path)))
        setError('')
        setSelected(previous => previous || result.files.find(f => /(^|\/)readme\.md$/i.test(f.path))?.path || '')
      })
      .catch(e => { if (current()) setError(errorMessage(e)) })
      .finally(() => { if (current()) setLoading(false) })
    return () => { abort.abort(); if (listingRequest.current === abort) listingRequest.current = null }
  }, [token, root.id, project.id, directory, revision, branchSwitching, repositoryRevision, repositoryBusy])

  useEffect(() => {
    if (!selected || branchSwitching || repositoryBusy) { setContent(null); setSourceLines(null); return }
    const abort = new AbortController()
    const generation = readGeneration.current
    const current = () => !abort.signal.aborted && latestContext.current === context && generation === readGeneration.current
    documentRequest.current = abort
    setContent(null); setSourceLines(null)
    api<{ path: string; content: string }>(token, `/projects/${project.id}/file?${query({ root_id: root.id, path: selected })}`, undefined, abort.signal)
      .then(result => { if (current()) { setContent(result.content); setError('') } })
      .catch(e => { if (current()) setError(errorMessage(e)) })
    return () => { abort.abort(); if (documentRequest.current === abort) documentRequest.current = null }
  }, [token, root.id, project.id, selected, revision, branchSwitching, repositoryRevision, repositoryBusy])

  useEffect(() => {
    if (!commentsAvailable || !selected || branchSwitching || repositoryBusy) { setSourceLines(null); return }
    const abort = new AbortController()
    const generation = readGeneration.current
    api<SourceLines>(token, `/projects/${project.id}/source-lines?${query({ root_id: root.id, path: selected })}`, undefined, abort.signal)
      .then(result => { if (!abort.signal.aborted && latestContext.current === context && generation === readGeneration.current) setSourceLines(result) })
      .catch(failure => { if (!abort.signal.aborted && latestContext.current === context && generation === readGeneration.current) setError(errorMessage(failure)) })
    return () => abort.abort()
  }, [token, project.id, root.id, selected, commentsAvailable, revision, repositoryRevision, repositoryBusy, branchSwitching])

  const markdown = /\.(md|markdown|mdown)$/i.test(selected)
  const location = linkedLocation?.path === selected ? linkedLocation : null
  const sourceWindow = sourceLineWindow(sourceLines?.lines.length || 0, location?.line)
  const structuredSource = commentsAvailable && sourceLines && (!location?.line || sourceWindow.found)
  return <div className="files-layout">
    {!externalToolbar && <div className={`panel-toolbar files-toolbar ${branchesAvailable ? 'with-branches' : ''}`}><RootSelect project={project} root={root} onChange={onRoot} disabled={branchSwitching} />{branchesAvailable && <BranchSelect key={context} token={token} projectId={project.id} rootId={root.id} revision={revision} onBeforeSwitch={() => { invalidateReads(); resetView(); setLoading(true); setBranchSwitching(true) }} onAfterSwitch={() => { invalidateReads(); resetView(); setLoading(true); setBranchSwitching(false); setRevision(value => value + 1) }} onExternalChange={() => { invalidateReads(); resetView(); setLoading(true); setRevision(value => value + 1) }} />}<span className="path-hint" title={root.path}>{root.path}</span><button className="icon-button" disabled={branchSwitching} onClick={() => { invalidateReads(); setLoading(true); setRevision(value => value + 1) }} title={branchesAvailable ? t("Обновить файлы и ветки") : t("Обновить файлы")} aria-label={branchesAvailable ? t("Обновить файлы и ветки") : t("Обновить файлы")}>↻</button></div>}
    {commentsAvailable && <div className="files-comments-tools"><button type="button" className="button secondary compact" aria-pressed={showComments} onClick={() => setShowComments(value => !value)}>{showComments ? t("Скрыть комментарии") : t("Комментарии")}</button>{selected && markdown && !raw && <button type="button" className="button secondary compact" onClick={() => { setRaw(true); setShowComments(true) }}>{t("Комментировать исходник")}</button>}</div>}
    {branchSwitching && <div className="files-branch-switching" role="status">{t("Переключаем ветку. Список файлов и открытый документ обновятся после завершения.")}</div>}
    {error && <div className="inline-error" role="alert">{t(error)}</div>}
    <div className={`files-body ${commentsAvailable && showComments ? 'files-with-comments' : ''}`}>
      <aside className="file-tree">
        <div className="file-tree-heading">{t("ФАЙЛЫ")}{' '}{loading && <span className="loading-dot" />}</div>
        {directory && <button className="file-row parent-folder" onClick={() => setDirectory(directory.split('/').slice(0, -1).join('/'))}>↰ ..</button>}
        {files.map(file => <button key={file.path} title={file.path} className={`file-row ${selected === file.path ? 'selected' : ''}`} onClick={() => { setLinkedLocation(null); setRaw(false); file.is_dir ? setDirectory(file.path) : setSelected(file.path) }}><span>{file.is_dir ? '▱' : /\.md$/i.test(file.path) ? 'M↓' : '≡'}</span><span>{file.path.split('/').pop()}</span></button>)}
        {!loading && !files.length && <p className="muted small padded">{t("В этой папке нет доступных файлов.")}</p>}
      </aside>
      <section className="file-document">
        {selected ? <><div className="document-heading"><span className="mono">{selected}{location?.line ? `:${location.line}${location.column ? ':' + location.column : ''}` : ''}</span>{markdown && <div className="segmented"><button className={!raw ? 'active' : ''} onClick={() => setRaw(false)}>{t("Просмотр")}</button><button className={raw ? 'active' : ''} onClick={() => setRaw(true)}>{t("Исходник")}</button></div>}</div>
          {content === null ? <div className="empty-state"><span className="muted">{error ? t("Файл недоступен") : t("Читаем файл…")}</span></div> : markdown && !raw ? <article className="markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml components={{
            img: ({ alt }) => <span className="image-placeholder">{t("Изображение:")}{' '}{alt || t("без подписи")}</span>,
            a: ({ href = '', children }) => {
              if (href.startsWith('#')) return <a href={href}>{children}</a>
              if (/^https?:\/\//i.test(href)) return <a href={href} target="_blank" rel="noreferrer noopener">{children} ↗</a>
              let target: string | null = null
              try { target = relativeTarget(selected, href) } catch { /* Invalid URL stays inert. */ }
              return target ? <button className="markdown-file-link" onClick={() => { setLinkedLocation(null); setSelected(target!); setDirectory(target!.split('/').slice(0, -1).join('/')) }}>{children}</button> : <span>{children}</span>
            },
          }}>{content}</ReactMarkdown></article> : structuredSource ? <><div className="source-review-notice">{(sourceLines!.lines_truncated || sourceLines!.lines.length > 2000) && <span>{t("Показана часть исходника: строки")}{' '}{sourceWindow.start + 1}–{sourceWindow.end}{t(". Комментарий к диапазону относится только к выбранным строкам.")}</span>}</div><CommentableCode key={context + '\0' + selected + '\0' + (location?.requestId || '')} token={token} projectId={project.id} rootId={root.id} path={selected} source="file" snapshot={sourceLines!.snapshot} lines={sourceLines!.lines.slice(sourceWindow.start, sourceWindow.end)} focusLine={location?.line} focusEndLine={location?.endLine} disabled={branchSwitching || repositoryBusy} onCreated={() => { setCommentsRevision(value => value + 1); setShowComments(true) }} /></> : location?.line ? <LinkedSourceView content={content} line={location.line} endLine={location.endLine} /> : <pre className="source-view">{content}</pre>}
        </> : <div className="empty-state"><div className="empty-symbol">M↓</div><h3>{t("Артефакты рядом с работой")}</h3><p>{t("Откройте Markdown, план или исходный файл.")}</p></div>}
      </section>
      {commentsAvailable && showComments && <ReviewCommentsPanel token={token} project={project} root={root} sessions={sessions} revision={commentsRevision + repositoryRevision} onLaunchForReview={onLaunchForReview} onOpenSession={onOpenSession} />}
    </div>
  </div>
}
