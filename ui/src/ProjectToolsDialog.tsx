import { useEffect, useState } from 'react'
import { api, errorMessage } from './api'
import type { Project } from './api'
import { t } from './i18n'
import Modal from './Modal'
import './project-tools.css'

type Server = { transport: 'http'; url: string } | { transport: 'stdio'; command: string; args: string[] }
type Skill = { description: string; instructions: string }
interface Profile { mcp: Record<string, Server>; skills: Record<string, Skill> }
interface Saved { revision: number; profile: Profile }
type Section = keyof Profile
type Export = Record<string, string>
const empty = { mcp: {}, skills: {} }
const harnesses = [['claude','Claude Code'], ['codex','Codex CLI'], ['agy','Google Antigravity'], ['hermes','Hermes']]

export default function ProjectToolsDialog({ token, project, onClose, onApplied }: {
  token: string; project: Project; onClose: () => void; onApplied: () => void;
}) {
  const [saved, setSaved] = useState<Saved | null>(null)
  const [draft, setDraft] = useState<Profile>(empty)
  const [section, setSection] = useState<Section>('mcp')
  const [rootId, setRootId] = useState(project.roots[0]?.id ?? '')
  const [harness, setHarness] = useState('claude')
  const [preview, setPreview] = useState<Export | null>(null)
  const [busy, setBusy] = useState('load')
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [newName, setNewName] = useState('')
  const endpoint = '/projects/' + encodeURIComponent(project.id) + '/tools'

  useEffect(() => {
    const abort = new AbortController()
    api<Saved>(token, endpoint, undefined, abort.signal).then(result => {
      setSaved(result)
      setDraft(result.profile)
    }).catch(failure => {
      if (!abort.signal.aborted) setError(errorMessage(failure))
    }).finally(() => {
      if (!abort.signal.aborted) setBusy('')
    })
    return () => abort.abort()
  }, [token, endpoint])

  const changed = saved !== null && JSON.stringify(draft) !== JSON.stringify(saved.profile)
  const edit = <K extends Section>(key: K, name: string, entry: Profile[K][string] | null) => {
    const entries = { ...draft[key] }
    if (entry === null) delete entries[name]
    setDraft({ ...draft, [key]: entry === null ? entries : { ...entries, [name]: entry } })
    setNotice('')
    setError('')
    setPreview(null)
  }
  const addEntry = () => {
    const name = newName.trim()
    if (!/^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/.test(name) || name.length > 64) {
      setError(t('В именах используйте строчные латинские буквы, цифры и одиночные дефисы.'))
      return
    }
    if (Object.hasOwn(draft[section], name)) {
      setError(t('В проекте уже есть запись с таким именем.'))
      return
    }
    if (section === 'mcp') edit('mcp', name, { transport: 'http', url: '' })
    else edit('skills', name, { description: '', instructions: '' })
    setNewName('')
  }

  const run = async (action: 'save' | 'apply' | 'preview') => {
    if (!saved || busy) return
    setBusy(action)
    setError('')
    setNotice('')
    try {
      if (action === 'save') {
        const result = await api<Saved>(token, endpoint, { expected_revision: saved.revision, profile: draft })
        setSaved(result)
        setDraft(result.profile)
        setNotice(t('Настройки инструментов проекта сохранены.'))
      } else if (action === 'apply') {
        await api(token, endpoint + '/apply', { root_id: rootId, expected_revision: saved.revision })
        setNotice(t('Файлы проекта обновлены. Запустите новую сессию, чтобы их загрузить.'))
        onApplied()
      } else {
        setPreview(await api<Export>(token, endpoint + '/export', harness))
      }
    } catch (failure) {
      setError(errorMessage(failure))
    } finally {
      setBusy('')
    }
  }

  return <Modal title={t('MCP и навыки')} subtitle={project.name} onClose={onClose} busy={!!busy}>
    <div className="project-tools">
      {saved && <>
        <div className="launch-defaults-scope" role="group" aria-label={t('MCP и навыки')}>
          <button type="button" className={section === 'mcp' ? 'active' : ''} disabled={!!busy} onClick={() => { setSection('mcp'); setNewName('') }}>{t('Серверы MCP')} · {Object.keys(draft.mcp).length}</button>
          <button type="button" className={section === 'skills' ? 'active' : ''} disabled={!!busy} onClick={() => { setSection('skills'); setNewName('') }}>{t('Навыки')} · {Object.keys(draft.skills).length}</button>
        </div>
        <fieldset disabled={!!busy}>
          <div className="project-tools-add">
            <label>{t('Имя')}<input value={newName} onChange={event => setNewName(event.target.value)} placeholder={section === 'mcp' ? 'project-docs' : 'review-code'} maxLength={64} onKeyDown={event => { if (event.key === 'Enter') addEntry() }} /></label>
            <button type="button" className="button secondary compact" disabled={!newName.trim()} onClick={addEntry}>{t('Добавить')}</button>
          </div>
          {section === 'mcp' ? Object.entries(draft.mcp).map(([name, server]) => <div className="project-tools-card" key={name}>
            <div className="project-tools-card-head"><strong>{name}</strong><button type="button" className="text-button" onClick={() => edit('mcp', name, null)}>{t('Убрать')}</button></div>
            <label>{t('Транспорт')}<select value={server.transport} onChange={event => edit('mcp', name, event.target.value === 'stdio' ? { transport: 'stdio', command: '', args: [] } : { transport: 'http', url: '' })}>
              <option value="http">Streamable HTTP</option><option value="stdio">stdio</option>
            </select></label>
            {server.transport === 'http' ? <label>{t('Адрес MCP')}<input type="url" value={server.url} placeholder="https://example.com/mcp" onChange={event => edit('mcp', name, { ...server, url: event.target.value })} /></label> : <>
              <label>{t('Исполняемый файл')}<input value={server.command} onChange={event => edit('mcp', name, { ...server, command: event.target.value })} /></label>
              <label>{t('Аргументы, по одному на строку')}<textarea rows={3} value={server.args.join('\n')} onChange={event => edit('mcp', name, { ...server, args: event.target.value.split('\n') })} /></label>
            </>}
          </div>) : Object.entries(draft.skills).map(([name, skill]) => <div className="project-tools-card" key={name}>
            <div className="project-tools-card-head"><strong>{name}</strong><button type="button" className="text-button" onClick={() => edit('skills', name, null)}>{t('Убрать')}</button></div>
            <label>{t('Описание')}<input value={skill.description} onChange={event => edit('skills', name, { ...skill, description: event.target.value })} /></label>
            <label>{t('Инструкции в Markdown')}<textarea rows={6} value={skill.instructions} onChange={event => edit('skills', name, { ...skill, instructions: event.target.value })} /></label>
          </div>)}
          {section === 'mcp' && <p className="muted">{t('CLI использует указанные команды и адреса MCP. Храните учётные данные в настройках агента или в его окружении.')}</p>}
        </fieldset>
        <div className="project-tools-export">
          <label>{t('Агент для экспорта')}<select disabled={!!busy} value={harness} onChange={event => { setHarness(event.target.value); setPreview(null) }}>{harnesses.map(([id, name]) => <option value={id} key={id}>{name}</option>)}</select></label>
          <button type="button" className="button secondary compact" disabled={!!busy || changed} onClick={() => void run('preview')}>{t('Предпросмотр экспорта')}</button>
          {preview && <div className="project-tools-preview">{Object.entries(preview).length ? Object.entries(preview).map(([path, contents]) => <details key={path}>
            <summary>{path}</summary>
            <pre tabIndex={0}>{contents}</pre>
            {navigator.clipboard && <button type="button" className="text-button" onClick={() => {
              void navigator.clipboard.writeText(contents).then(
                () => setNotice(t('Содержимое файла скопировано.')),
                () => setError(t('Не удалось скопировать файл. Выделите текст в предпросмотре.')),
              )
            }}>{t('Скопировать содержимое файла')}</button>}
          </details>) : <p className="muted">{t('Пока нет файлов для экспорта.')}</p>}</div>}
        </div>
        <div className="project-tools-apply">
          <label>{t('Рабочая папка')}<select value={rootId} disabled={!!busy} onChange={event => setRootId(event.target.value)}>{project.roots.map(root => <option value={root.id} key={root.id}>{root.name}</option>)}</select></label>
          <button type="button" className="button secondary compact" disabled={!!busy || changed || !rootId || !saved.revision} onClick={() => void run('apply')}>{t('Применить к папке')}</button>
          <p className="muted">{t('Применение записывает файлы для Claude Code, Codex и Antigravity. Для Hermes доступен предпросмотр экспорта.')}</p>
        </div>
      </>}
      {notice && <div className="success-notice" role="status">{notice}</div>}
      {error && <div className="inline-error" role="alert">{error}</div>}
      <div className="modal-actions">
        <button type="button" className="button secondary" disabled={!!busy} onClick={onClose}>{t('Закрыть')}</button>
        <button type="button" className="button primary" disabled={!!busy || !changed} onClick={() => void run('save')}>{busy === 'save' ? t('Сохраняем…') : t('Сохранить настройки')}</button>
      </div>
    </div>
  </Modal>
}
