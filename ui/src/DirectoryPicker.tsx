import { t } from './i18n.ts'
import { useEffect, useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { api, errorMessage, query } from './api'

interface DirectoryListing {
  path: string
  home: string
  parent: string | null
  entries: { name: string; path: string }[]
  truncated: boolean
}

export interface DirectorySelection { path: string; parent: string | null }

function breadcrumbs(path: string): { label: string; path: string }[] {
  if (!path) return []
  const separator = !path.startsWith('/') && path.includes('\\') ? '\\' : '/'
  const parts = path.split(separator).filter(Boolean)
  const prefix = path.startsWith(separator + separator) ? separator + separator : path.startsWith(separator) ? separator : ''
  const result = prefix ? [{ label: prefix, path: prefix }] : []
  let current = prefix
  for (const part of parts) {
    current += (current && !current.endsWith(separator) ? separator : '') + part
    if (/^[A-Za-z]:$/.test(current)) current += separator
    result.push({ label: part, path: current })
  }
  return result
}

export default function DirectoryPicker({ token, initialPath, selectedPaths, onSelect, onCancel }: {
  token: string
  initialPath?: string
  selectedPaths: string[]
  onSelect: (selection: DirectorySelection) => void
  onCancel: () => void
}) {
  const [requestedPath, setRequestedPath] = useState(initialPath || '')
  const [typedPath, setTypedPath] = useState(initialPath || '')
  const [listing, setListing] = useState<DirectoryListing | null>(null)
  const [home, setHome] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [showHidden, setShowHidden] = useState(false)
  const [search, setSearch] = useState('')
  const [revision, setRevision] = useState(0)
  const generation = useRef(0)

  useEffect(() => {
    const abort = new AbortController()
    const requestGeneration = ++generation.current
    setListing(null); setLoading(true); setError('')
    const params = query({ ...(requestedPath ? { path: requestedPath } : {}), show_hidden: String(showHidden) })
    api<DirectoryListing>(token, '/directories?' + params, undefined, abort.signal)
      .then(result => {
        if (abort.signal.aborted || requestGeneration !== generation.current) return
        setListing(result); setHome(result.home); setTypedPath(result.path)
      })
      .catch(e => { if (!abort.signal.aborted && requestGeneration === generation.current) setError(errorMessage(e)) })
      .finally(() => { if (!abort.signal.aborted && requestGeneration === generation.current) setLoading(false) })
    return () => abort.abort()
  }, [token, requestedPath, showHidden, revision])

  const navigate = (path: string) => {
    // Invalidate the old selection in this event, before the next effect runs.
    generation.current += 1
    setSearch('')
    setListing(null); setLoading(true); setError(''); setTypedPath(path)
    setRequestedPath(path); setRevision(v => v + 1)
  }
  const go = (event: FormEvent) => { event.preventDefault(); if (typedPath.trim()) navigate(typedPath) }
  const duplicate = !!listing && selectedPaths.includes(listing.path)
  const canSelect = !!listing && !loading && !error && !duplicate && selectedPaths.length < 12
  const currentPath = listing?.path || requestedPath
  const needle = search.trim().normalize('NFC').toLocaleLowerCase()
  const visibleEntries = listing?.entries.filter(entry => entry.name.normalize('NFC').toLocaleLowerCase().includes(needle)) || []

  return <div className="directory-picker">
    <div className="directory-picker-heading"><button type="button" className="directory-back" onClick={onCancel}>{t("← К проекту")}</button><span>{t("Папки на этом компьютере")}</span></div>
    <form className="directory-address" onSubmit={go}><label htmlFor="directory-path">{t("Текущий путь")}</label><div><input id="directory-path" autoFocus value={typedPath} onChange={e => setTypedPath(e.target.value)} placeholder={t("Домашняя папка")} autoComplete="off" spellCheck={false} /><button type="submit" className="button secondary compact" disabled={!typedPath.trim()}>{t("Перейти")}</button></div></form>
    <div className="directory-navigation"><button type="button" className="button secondary compact" onClick={() => navigate(home)} disabled={loading}>{t("⌂ Домой")}</button><button type="button" className="button secondary compact" disabled={loading || !listing?.parent} onClick={() => { if (listing?.parent) navigate(listing.parent) }}>{t("↑ На уровень выше")}</button><label className="directory-hidden-toggle"><input type="checkbox" checked={showHidden} onChange={e => { generation.current += 1; setListing(null); setLoading(true); setError(''); setShowHidden(e.target.checked) }} />{t("Скрытые")}</label></div>
    <nav className="directory-breadcrumbs" aria-label={t("Путь к текущей папке")}>{breadcrumbs(currentPath).map((item, index) => <span key={item.path}>{index > 0 && <i> / </i>}<button type="button" disabled={loading} onClick={() => navigate(item.path)} title={item.path}>{item.label}</button></span>)}</nav>
    <div className="directory-search"><label htmlFor="directory-search">{t("Поиск по названию")}</label><div><input id="directory-search" type="search" value={search} onChange={e => setSearch(e.target.value)} placeholder={t("Найти папку в текущем каталоге…")} autoComplete="off" spellCheck={false} aria-controls="directory-entries" disabled={loading || !!error} />{search && <button type="button" className="icon-button" onClick={() => setSearch('')} aria-label={t("Очистить поиск")}>×</button>}</div>{needle && !loading && !error && <span role="status">{t("Найдено")}{' '}{visibleEntries.length}{' '}{t("из")}{' '}{listing?.entries.length || 0}</span>}</div>
    <div id="directory-entries" className="directory-entries" aria-busy={loading} aria-label={t("Папки")}>
      {loading ? <div className="directory-empty" role="status"><span className="loading-dot" />{t("Открываем папку…")}</div> : error ? <div className="directory-load-error" role="alert"><strong>{t("Не удалось открыть папку")}</strong><p>{t(error)}</p><p>{t("Выберите другой путь или проверьте доступ к папке на этом компьютере.")}</p><button type="button" className="button secondary compact" onClick={() => navigate(requestedPath)}>{t("Повторить")}</button></div> : visibleEntries.length ? visibleEntries.map(entry => <button type="button" className="directory-entry" key={entry.path} onClick={() => navigate(entry.path)} title={entry.path}><span aria-hidden="true">▱</span><span>{entry.name}</span><span aria-hidden="true">›</span></button>) : <div className="directory-empty"><span aria-hidden="true">▱</span><p>{needle ? t("Папки с таким названием не найдены.") : t("Вложенных папок нет.")}</p><small>{needle ? t("Измените запрос или очистите поиск.") : t("Эту папку всё равно можно выбрать.")}</small></div>}
    </div>
    {listing?.truncated && <div className="notice directory-notice">{t("Показана только часть папок из-за лимита списка. Введите полный путь выше, чтобы открыть папку, которой нет в списке.")}</div>}
    {duplicate && <div className="notice directory-notice">{t("Эта папка уже добавлена в проект. Выберите другую или вернитесь к проекту.")}</div>}
    <div className="directory-selection"><span>{t("Выбираемая папка")}</span><code title={currentPath}>{currentPath || t("Домашняя папка")}</code></div>
    <div className="modal-actions"><button type="button" className="button secondary" onClick={onCancel}>{t("Отмена")}</button><button type="button" className="button primary" disabled={!canSelect} onClick={() => { if (canSelect && listing) onSelect({ path: listing.path, parent: listing.parent }) }}>{duplicate ? t("Уже добавлена") : t("Выбрать эту папку")}</button></div>
  </div>
}
