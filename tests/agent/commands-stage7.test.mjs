import assert from 'node:assert/strict'
import test from 'node:test'
import {fixture} from './helpers.mjs'
import fs from 'node:fs'
import path from 'node:path'
test('A6 clear resets context and cls keeps the original display-only action', async t => {
 const f=await fixture(t,[{text:'done'}]);const run=await f.run()
 assert.equal((await f.request('runSlash',{text:'/cls',workspace:f.workspace})).action,'clear')
 assert.equal((await f.request('getState')).sessionId,run.id)
 assert.equal((await f.request('runSlash',{text:'/clear',workspace:f.workspace})).action,'new')
 assert.equal((await f.request('getState')).sessionId,null)
 assert.equal((await f.request('getSession',{sessionId:run.id})).summary.session_id,run.id)
})
test('A6 shared diagnostic commands report actual state and init never overwrites', async t => {
 const f=await fixture(t,[{text:'done'}]);await f.run()
 for(const text of ['/cost','/context','/doctor','/agents','/mcp','/memory']) {
  assert.ok((await f.request('runSlash',{text,workspace:f.workspace})).message)
 }
 await f.request('runSlash',{text:'/init',workspace:f.workspace})
 const file=path.join(f.workspace,'AGENTS.md'),before=fs.readFileSync(file,'utf8')
 await assert.rejects(f.request('runSlash',{text:'/init',workspace:f.workspace}),/已存在/)
 assert.equal(fs.readFileSync(file,'utf8'),before)
 await assert.rejects(f.request('runSlash',{text:'/rewind',workspace:f.workspace}),/G3/)
 await assert.rejects(f.request('runSlash',{text:'/clear surprise',workspace:f.workspace}),/参数/)
})
