import { useEffect, useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { api, errorMessage, query } from './api'
import { t } from './i18n.ts'
import Modal from './Modal'
import { envFileBreadcrumbs, filterEnvFileEntries, selectedEnvFile } from './env-file-picker.ts'
import type { EnvFileListing } from './env-file-picker.ts'
import './env-file-picker.css'

interface Props { token: string; initialPath: string; onSelect: (path: string) => void; onClose: () => void }

export default function EnvFilePicker(props: Props) {
  return <EnvFilePickerForm key={props.token + '\0' + props.initialPath} {...props} />
}

function EnvFilePickerForm({ token, initialPath, onSelect, onClose }: Props) {
  const [requestedPath, setRequestedPath] = useState(initialPath)
  const [typedPath, setTypedPath] = useState(initialPath)
  const [listing, setListing] = useState<EnvFileListing | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [search, setSearch] = useState('')
  const [selected, setSelected] = useState<string | null>(null)
  const [revision, setRevision] = useState(0)
  const generation = useRef(0)

  useEffect(() => {
    const abort = new AbortController()
    const current = ++generation.current
    setListing(null); setSelected(null); setLoading(true); setError('')
    api<EnvFileListing>(token, '/environment-files?' + query(requestedPath ? { path: requestedPath } : {}), undefined, abort.signal)
      .then(result => {
        if (abort.signal.aborted || current !== generation.current) return
        setListing(result); setTypedPath(result.path)
      })
      .catch(failure => { if (!abort.signal.aborted && current === generation.current) setError(errorMessage(failure)) })
      .finally(() => { if (!abort.signal.aborted && current === generation.current) setLoading(false) })
    return () => abort.abort()
  }, [token, requestedPath, revision])

  const navigate = (path: string) => {
    // Invalidate selection synchronously, before an in-flight request can finish.
    generation.current++
    setSelected(null); setListing(null); setLoading(true); setError(''); setSearch('')
    setTypedPath(path); setRequestedPath(path); setRevision(value => value + 1)
  }
  const go = (event: FormEvent) => { event.preventDefault(); if (typedPath.length) navigate(typedPath) }
  const selection = selectedEnvFile(listing, selected, loading, error)
  const visibleEntries = filterEnvFileEntries(listing?.entries || [], search)
  const currentPath = listing?.path || requestedPath
  const choose = () => { if (selection) onSelect(selection) }

  return <Modal title={t('Выбрать ENV-файл')} subtitle={t('Выберите файл с переменными окружения для этой сессии.')} onClose={onClose}>
    <div className="directory-picker env-file-picker">
      <div className="directory-picker-heading"><button type="button" className="directory-back" onClick={onClose}>{t('← К сессии')}</button><span>{t('Файлы на этом компьютере')}</span></div>
      <form className="directory-address" onSubmit={go}>
        <label htmlFor="env-file-directory">{t('Текущая папка')}</label>
        <div><input id="env-file-directory" autoFocus value={typedPath} onChange={event => { setTypedPath(event.target.value); setSelected(null) }} placeholder={t('Домашняя папка')} autoComplete="off" spellCheck={false} /><button type="submit" className="button secondary compact" disabled={!typedPath.length}>{t('Перейти')}</button></div>
      </form>
      <div className="directory-navigation"><button type="button" className="button secondary compact" disabled={loading} onClick={() => navigate(listing?.home || '')}>{t('⌂ Домой')}</button><button type="button" className="button secondary compact" disabled={loading || !listing?.parent} onClick={() => { if (listing?.parent) navigate(listing.parent) }}>{t('↑ На уровень выше')}</button><span className="env-picker-hidden-note">{t('Скрытые файлы показаны')}</span></div>
      <nav className="directory-breadcrumbs" aria-label={t('Путь к текущей папке')}>{envFileBreadcrumbs(currentPath).map((item, index) => <span key={item.path}>{index > 0 && <i> / </i>}<button type="button" disabled={loading} onClick={() => navigate(item.path)} title={item.path}>{item.label}</button></span>)}</nav>
      <div className="directory-search"><label htmlFor="env-file-search">{t('Поиск по названию')}</label><div><input id="env-file-search" type="search" value={search} onChange={event => { setSearch(event.target.value); setSelected(null) }} placeholder={t('Найти файл или папку…')} autoComplete="off" spellCheck={false} aria-controls="env-file-entries" disabled={loading || !!error} />{search && <button type="button" className="icon-button" onClick={() => { setSearch(''); setSelected(null) }} aria-label={t('Очистить поиск')}>×</button>}</div>{search && !loading && !error && <span role="status">{t('Найдено')}{' '}{visibleEntries.length}{' '}{t('из')}{' '}{listing?.entries.length || 0}</span>}</div>
      <div id="env-file-entries" className="directory-entries env-picker-entries" aria-busy={loading} aria-label={t('Файлы и папки')}>
        {loading ? <div className="directory-empty" role="status"><span className="loading-dot" />{t('Открываем папку…')}</div> : error ? <div className="directory-load-error" role="alert"><strong>{t('Не удалось открыть папку')}</strong><p>{t(error)}</p><p>{t('Выберите другой путь или проверьте доступ к папке на этом компьютере.')}</p><button type="button" className="button secondary compact" onClick={() => navigate(requestedPath)}>{t('Повторить')}</button></div> : visibleEntries.length ? visibleEntries.map(entry => <button type="button" key={entry.path} className={`directory-entry env-picker-entry ${!entry.is_dir && selection === entry.path ? 'selected' : ''}`} aria-pressed={entry.is_dir ? undefined : selection === entry.path} onClick={() => { if (entry.is_dir) navigate(entry.path); else setSelected(entry.path) }} title={entry.path}><span aria-hidden="true">{entry.is_dir ? '▱' : '≡'}</span><span>{entry.name}</span><small>{entry.is_dir ? t('Папка') : entry.size !== undefined ? entry.size >= 1024 ? (entry.size / 1024).toFixed(1) + ' KiB' : entry.size + ' B' : t('Файл')}</small><span aria-hidden="true">{entry.is_dir ? '›' : selection === entry.path ? '✓' : ''}</span></button>) : <div className="directory-empty"><span aria-hidden="true">≡</span><p>{search ? t('Файлы и папки с таким названием не найдены.') : t('В этой папке нет доступных файлов или папок.')}</p><small>{search ? t('Измените запрос или очистите поиск.') : t('Выберите другую папку.')}</small></div>}
      </div>
      {listing?.truncated && <div className="notice directory-notice">{t('Показана только часть списка. Укажите нужную папку в поле пути.')}</div>}
      <p className="env-picker-privacy">{t('Содержимое файла здесь не открывается. Переменные будут загружены при запуске сессии.')}</p>
      <div className="directory-selection"><span>{t('Выбранный ENV-файл')}</span>{selection ? <code title={selection}>{selection}</code> : <span className="env-picker-no-selection">{t('Выберите ENV-файл в списке.')}</span>}</div>
      <div className="modal-actions"><button type="button" className="button secondary" onClick={onClose}>{t('Отмена')}</button><button type="button" className="button primary" disabled={!selection} onClick={choose}>{t('Использовать этот файл')}</button></div>
    </div>
  </Modal>
}
