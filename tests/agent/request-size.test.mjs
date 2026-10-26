import assert from 'node:assert/strict'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'
import {setTimeout as delay} from 'node:timers/promises'

test('B19: a 5MB request receives its correlated error before the request timeout',async t=>{
 const f=await fixture(t)
 await f.request('initialize')
 const start=performance.now()
 await assert.rejects(Promise.race([
   f.request('getState',{text:'x'.repeat(5_000_000)}),
   delay(750).then(()=>{throw Error('no correlated size error')})
 ]),/request exceeds the size limit/)
 assert.ok(performance.now()-start<750)
 assert.equal((await f.request('getState')).protocolVersion,2)
})
