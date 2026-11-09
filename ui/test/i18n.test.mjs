import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import { createRequire } from 'node:module'
import { getLocale, isLocale, localeTag, LOCALE_KEY, messages, setLocale, storedLocale, subscribeLocale, t } from '../src/i18n.ts'
const require = createRequire(import.meta.url)
const ts = require('typescript')
const placeholders = text => [...new Set([...text.matchAll(/\{(\w+)\}/g)].map(match => match[1]))].sort()
function withStorage(storage, work) {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, get: () => storage })
  try { return work() } finally { if (descriptor) Object.defineProperty(globalThis, 'localStorage', descriptor); else delete globalThis.localStorage }
}

test('clean startup defaults to English; browser language does not choose the app language', () => {
  assert.equal(getLocale(), 'en')
  assert.equal(storedLocale(), 'en')
  assert.equal(t('Проекты'), 'Projects')
  assert.equal(localeTag(), 'en-US')
})
test('only supported stored languages are restored, with English fallback', () => {
  for (const [stored, expected] of [[null,'en'],['','en'],['fr','en'],['es','es'],['ru','ru'],['en','en']]) {
    withStorage({ getItem: key => { assert.equal(key, LOCALE_KEY); return stored } }, () => assert.equal(storedLocale(), expected))
  }
  assert.equal(isLocale('de'), false)
})
test('a language selection is durable and never touches credentials or project preferences', () => {
  const values = new Map([['mpac-token','private-token'],['mpac-selected-project','project-id']])
  let changes = 0
  const unsubscribe = subscribeLocale(() => changes++)
  withStorage({ getItem: key => values.get(key), setItem: (key,value) => values.set(key,value) }, () => {
    setLocale('es'); assert.equal(getLocale(), 'es'); assert.equal(t('Проекты'), 'Proyectos'); assert.equal(storedLocale(),'es')
    setLocale('es'); assert.equal(changes,1)
    setLocale('ru'); assert.equal(t('Проекты'),'Проекты'); assert.equal(localeTag(),'ru-RU')
    assert.equal(values.get('mpac-token'),'private-token');assert.equal(values.get('mpac-selected-project'),'project-id')
  })
  unsubscribe(); setLocale('en',false)
})
test('blocked storage does not prevent changing language in memory', () => {
  withStorage({ getItem: () => { throw new Error('blocked') }, setItem: () => { throw new Error('blocked') } }, () => {
    assert.equal(storedLocale(),'en'); setLocale('es'); assert.equal(t('Файлы'),'Archivos');setLocale('en')
  })
})
test('all catalog messages have English/Spanish translations and preserve named parameters', () => {
  assert.ok(Object.keys(messages).length > 800)
  for (const [key, translations] of Object.entries(messages)) {
    for (const locale of ['en','es']) {
      assert.equal(typeof translations[locale],'string',`${key}: ${locale}`)
      assert.ok(translations[locale].length,`${key}: ${locale}`)
      assert.doesNotMatch(translations[locale],/[А-Яа-яЁё]/,`${key}: ${locale}`)
      assert.deepEqual(placeholders(translations[locale]),placeholders(key),`${key}: ${locale}`)
    }
  }
})
test('interpolated user names, paths and code remain literal through language changes', () => {
  const value = 'Файлы / src/Ошибка {p1} <script>$&</script>.md'
  setLocale('en',false)
  const notice = t('Проект «{p0}» восстановлен.',{p0:value})
  assert.ok(notice.includes(value))
  setLocale('es',false);assert.equal(t(notice),t('Проект «{p0}» восстановлен.',{p0:value}))
  setLocale('ru',false);assert.equal(t(notice),`Проект «${value}» восстановлен.`)
  setLocale('en',false);assert.equal(t(value),value)
})
test('specific cached messages win over generic templates after changing locale', () => {
  setLocale('en',false)
  const cached = t('Старая строка {p0}',{p0:4})
  setLocale('es',false)
  assert.equal(t(cached),t('Старая строка {p0}',{p0:4}))
  assert.ok(!t(cached).includes('Old'))
  setLocale('ru',false);assert.equal(t(cached),'Старая строка 4');setLocale('en',false)
})
test('known daemon errors are localized without changing native API payloads', () => {
  const raw = 'CLI завершился с ошибкой'
  for (const locale of ['en','es','ru']) {
    setLocale(locale,false)
    const localized = t(raw)
    if (locale !== 'ru') assert.doesNotMatch(localized,/[А-Яа-яЁё]/)
    else assert.equal(localized,raw)
    assert.equal(raw,'CLI завершился с ошибкой')
  }
  setLocale('en',false)
})
test('all literal application translation calls have catalog entries', () => {
  const missing=[]
  for (const file of fs.readdirSync(new URL('../src/',import.meta.url))) {
    if (!/\.tsx?$/.test(file)) continue
    const url = new URL('../src/'+file,import.meta.url), source=fs.readFileSync(url,'utf8')
    const tree=ts.createSourceFile(file,source,ts.ScriptTarget.Latest,true,file.endsWith('.tsx')?ts.ScriptKind.TSX:ts.ScriptKind.TS)
    const visit=node=>{
      if (ts.isCallExpression(node)&&ts.isIdentifier(node.expression)&&node.expression.text==='t'&&node.arguments[0]&&ts.isStringLiteral(node.arguments[0])) {
        const key=node.arguments[0].text;if (!Object.hasOwn(messages,key)) missing.push(`${file}:${key}`)
      }
      ts.forEachChild(node,visit)
    }
    visit(tree)
  }
  assert.deepEqual(missing,[])
})
