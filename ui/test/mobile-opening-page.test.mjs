import test from 'node:test'
import assert from 'node:assert/strict'
import {readFileSync} from 'node:fs'
import vm from 'node:vm'
import {nextIncomingConnection} from '../src/native-mobile.ts'
const source=readFileSync(new URL('../../web/mobile-open/open/open.js',import.meta.url),'utf8')
function page(hash=''){
 const elements=new Map(),copies=[]
 const querySelector=id=>{if(!elements.has(id))elements.set(id,{href:'mpac://open',hidden:true,events:{},addEventListener(name,fn){this.events[name]=fn},removeAttribute(name){delete this[name]},setAttribute(name,value){this[name]=value}});return elements.get(id)}
 vm.runInNewContext(source,{document:{querySelector},location:{hash},navigator:{clipboard:{writeText:async text=>copies.push(text)}},window:{addEventListener(){}},URL,URLSearchParams})
 return{elements,copies}
}
const invitation='mpac://pair?v=1&endpoint='+encodeURIComponent('https://192.168.1.4:4330/')+'#ca=AQID&invite='+'a'.repeat(64)
test('permanent app link opens MPAC without a pairing token or network request',()=>{
 const p=page();assert.equal(p.elements.get('#open').href,'mpac://open');assert.deepEqual(p.copies,[])
})
test('opening page preserves the complete invitation locally and copies only on click',async()=>{
 const p=page('#'+new URLSearchParams({link:invitation}));assert.equal(p.elements.get('#open').href,invitation);assert.equal(p.elements.get('#computer').textContent,'https://192.168.1.4:4330');assert.deepEqual(p.copies,[])
 await p.elements.get('#copy').events.click();assert.deepEqual(p.copies,[invitation])
})
test('malformed links cannot navigate, run commands, or select an external server',()=>{
 for(const value of ['javascript:alert(1)','mpac://exec?cmd=run',invitation+'&extra=1',invitation.replace('192.168.1.4','public.example.com')]){
  const p=page('#'+new URLSearchParams({link:value}));assert.equal(p.elements.get('#open').href,undefined);assert.equal(p.elements.get('#copy').disabled,true)
 }
 const p=page('#'+new URLSearchParams({link:invitation})+'&link=duplicate');assert.equal(p.elements.get('#copy').disabled,true)
})
test('a malformed later URL cannot erase a valid review candidate; a newer invitation can replace it',()=>{
 const first={kind:'pair',link:invitation,endpoint:'https://192.168.1.4:4330/'}
 assert.equal(nextIncomingConnection(first,{kind:'invalid'}),first)
 const second={...first,link:invitation.replace('a'.repeat(64),'b'.repeat(64))}
 assert.equal(nextIncomingConnection(first,second),second)
})
