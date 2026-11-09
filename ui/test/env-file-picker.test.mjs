import assert from 'node:assert/strict'
import test from 'node:test'
import { envFileBreadcrumbs, filterEnvFileEntries, selectedEnvFile } from '../src/env-file-picker.ts'

test('arbitrary files including hidden and extensionless names remain selectable with literal path whitespace', () => {
  const entries = [
    { name: '.env.zai', path: '/project/.env.zai', is_dir: false },
    { name: ' production variables ', path: '/project/ production variables ', is_dir: false },
    { name: 'config', path: '/project/config', is_dir: true },
  ]
  const listing = { path: '/project', home: '/home', parent: '/', entries, truncated: false }
  assert.equal(selectedEnvFile(listing, '/project/ production variables ', false, ''), '/project/ production variables ')
  assert.equal(selectedEnvFile(listing, '/project/.env.zai', false, ''), '/project/.env.zai')
  assert.equal(selectedEnvFile(listing, '/project/config', false, ''), null)
  assert.deepEqual(filterEnvFileEntries(entries, '').map(entry => entry.path), ['/project/config', '/project/ production variables ', '/project/.env.zai'])
})
test('loading, errors and stale directory listings cannot select a previous file', () => {
  const listing = { path: '/one', home: '/', parent: '/', entries: [{ name: '.env', path: '/one/.env', is_dir: false }], truncated: false }
  assert.equal(selectedEnvFile(null, '/one/.env', false, ''), null)
  assert.equal(selectedEnvFile(listing, '/one/.env', true, ''), null)
  assert.equal(selectedEnvFile(listing, '/one/.env', false, 'failed'), null)
  assert.equal(selectedEnvFile({ ...listing, entries: [] }, '/one/.env', false, ''), null)
  assert.equal(selectedEnvFile(listing, null, false, ''), null)
})
test('search is case and Unicode composition insensitive without mutating or trimming file names', () => {
  const entry = { name: ' Production-Cafe\u0301 ', path: '/p/ Production-Cafe\u0301 ', is_dir: false }
  const entries = [entry]
  assert.equal(filterEnvFileEntries(entries, 'CAFÉ')[0], entry)
  assert.equal(filterEnvFileEntries(entries, ' ')[0], entry)
  assert.equal(filterEnvFileEntries(entries, 'missing').length, 0)
  assert.deepEqual(entries, [entry])
})
test('breadcrumbs preserve directory names and platform roots', () => {
  assert.deepEqual(envFileBreadcrumbs('/Users/me/project '), [
    { label: '/', path: '/' }, { label: 'Users', path: '/Users' }, { label: 'me', path: '/Users/me' }, { label: 'project ', path: '/Users/me/project ' },
  ])
  assert.equal(envFileBreadcrumbs('C:\\Projects\\folder ').at(-1).path, 'C:\\Projects\\folder ')
  assert.equal(envFileBreadcrumbs('\\\\server\\share\\folder').at(-1).path, '\\\\server\\share\\folder')
  assert.deepEqual(envFileBreadcrumbs(''), [])
})
