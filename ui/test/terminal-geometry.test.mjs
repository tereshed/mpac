import test from 'node:test'
import assert from 'node:assert/strict'
import { applyTerminalOutput } from '../src/terminal-geometry.ts'
import { terminalReadingText, terminalLiveText } from '../src/terminal-mobile.ts'

test('a phone replays historical resizes in byte order, including a split Unicode character', async () => {
  const bytes = Buffer.from('Привет\r\n\x1b[140GUpdate installed')
  const calls = []
  const sink = { cols: 40, rows: 20, resize(cols, rows) { this.cols = cols; this.rows = rows; calls.push([cols, rows]) }, write(data, done) { calls.push(Buffer.from(data)); done() } }
  assert.equal(await applyTerminalOutput({data_base64: bytes.toString('base64'), geometry: {cols:100,rows:24}, resizes:[{offset:3,cols:180,rows:50}]}, sink, true), true)
  assert.deepEqual(calls.filter(Array.isArray), [[100,24],[180,50]])
  assert.deepEqual(Buffer.concat(calls.filter(Buffer.isBuffer)), bytes)
  assert.equal(calls[1].length, 3)
})
test('metadata-only output changes the screen without inventing input or output', async () => {
  const calls = []
  const sink = {cols:100,rows:24,resize(...size){calls.push(size)},write(){assert.fail('empty output must not write')}}
  await applyTerminalOutput({data_base64:'',geometry:{cols:160,rows:40},resizes:[]},sink,true)
  assert.deepEqual(calls, [[160,40]])
})
test('unknown hosts fail closed; malformed geometry never partially updates a terminal', async () => {
  const sink = {cols:40,rows:20,resize(){assert.fail('unexpected resize')},write(){assert.fail('unexpected output')}}
  assert.equal(await applyTerminalOutput({data_base64:'YQ=='},sink,true),false)
  for (const change of [{geometry:{cols:0,rows:24}}, {resizes:[{offset:2,cols:100,rows:24}]}, {resizes:[{offset:0,cols:100,rows:24}]}, {resizes:[{offset:1,cols:301,rows:24}]}]) {
    await assert.rejects(applyTerminalOutput({data_base64:'YQ==',geometry:{cols:100,rows:24},...change},sink,true),/размеры/)
  }
})
test('desktop controlling viewers retain their own layout on older hosts', async () => {
  const calls=[]
  await applyTerminalOutput({data_base64:'YQ=='},{cols:120,rows:40,resize(){assert.fail()},write(data,done){calls.push(Buffer.from(data).toString());done()}},false)
  assert.deepEqual(calls,['a'])
})
test('read view joins soft wraps without joining source lines or losing spaces and Unicode', () => {
  const rows=[['Привет ',false],['мир 👋',true],['  const x = 1',false],['next',false]]
  const buffer={length:rows.length,getLine(i){return rows[i]&&{isWrapped:rows[i][1],translateToString(){return rows[i][0]}}}}
  assert.equal(terminalReadingText(buffer),'Привет мир 👋\n  const x = 1\nnext')
  assert.ok(terminalReadingText(buffer,5000,10).length<=10)
})
test('a reply taller than the phone screen is readable from its first line in the normal buffer', () => {
  const rows=['Начало ответа 👋', '  ```rust', ...Array.from({length:100},(_,i)=>`    line ${i}`), '  ```', 'Конец ответа', '> ']
  const buffer={type:'normal',baseY:rows.length-24,length:rows.length,getLine(i){return rows[i]===undefined?undefined:{isWrapped:false,translateToString(){return rows[i]}}}}
  assert.equal(terminalLiveText(buffer,24), rows.join('\n').trimEnd())
  assert.ok(!terminalLiveText({...buffer,type:'alternate'},24).includes('Начало'))
})
test('live reading removes terminal vertical padding without flattening code indentation', () => {
  const rows=['answer','','','','','  code','','> ']
  const buffer={type:'alternate',baseY:0,length:rows.length,getLine(i){return {isWrapped:false,translateToString(){return rows[i]}}}}
  assert.equal(terminalLiveText(buffer,rows.length),'answer\n\n  code\n\n>')
})
