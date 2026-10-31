import assert from 'node:assert/strict'
import test from 'node:test'
import {fixture} from './helpers.mjs'
test('UX1 resume lists recent sessions and accepts any unique prefix', async t => {
 const f=await fixture(t,[{text:'done'}]);const run=await f.run()
 const listed=await f.request('runSlash',{text:'/resume',workspace:f.workspace})
 assert.match(listed.message, new RegExp(run.id.slice(0,8)))
 const selected=await f.request('runSlash',{text:`/resume ${run.id.slice(0,5)}`,workspace:f.workspace})
 assert.equal(selected.sessionId,run.id)
 await assert.rejects(f.request('runSlash',{text:'/resume missing',workspace:f.workspace}),/不存在/)
})
