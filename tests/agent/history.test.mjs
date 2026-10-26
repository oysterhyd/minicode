import assert from 'node:assert/strict'
import {test} from 'node:test'
import {DatabaseSync} from 'node:sqlite'
import {fixture, connect} from './helpers.mjs'

test('B14: sessions over 64 MiB open in bounded pages and older history is lossless', async t => {
  const f = await fixture(t, [{text:'initial'}])
  const first = await f.run()
  const db = new DatabaseSync(f.db)
  const message = db.prepare('INSERT INTO messages VALUES(?,?,?,?)')
  const event = db.prepare('INSERT INTO events VALUES(?,?,?,?,?)')
  const payload = JSON.stringify({output_preview:'x'.repeat(512 * 1024)})
  db.exec('BEGIN')
  for (let seq = 2; seq < 242; seq++) message.run(first.id,seq,'user',JSON.stringify([{type:'text',text:`history-${seq}`}]))
  for (let seq = 100; seq < 240; seq++) event.run(first.id,seq,'large_fixture',new Date().toISOString(),payload)
  db.exec('COMMIT'); db.close()
  const selected = await connect(f).request('selectSession', {clientKey:'paged',sessionId:first.id,deferRuntime:true})
  assert.equal(selected.sessionId, first.id)
  assert.equal(selected.contextTokens, 0, 'selection must defer Runtime history loading')
  let page = await f.request('getSession', {sessionId:first.id})
  assert.ok(page.messages.length <= 100, 'first screen read the entire message history')
  assert.ok(Buffer.byteLength(JSON.stringify(page)) < 4 * 1024 * 1024, 'first screen read the entire event history')
  assert.ok(page.cursor && (page.cursor.messages !== null || page.cursor.events !== null))
  const messages = [...page.messages], events = [...page.events]
  while (page.cursor.messages !== null || page.cursor.events !== null) {
    page = await f.request('getSessionPage', {sessionId:first.id, cursor:page.cursor, limit:100})
    messages.unshift(...page.messages); events.unshift(...page.events)
  }
  assert.equal(messages.length, 242)
  assert.equal(new Set(messages.slice(2).map(m=>m.content[0].text)).size, 240)
  assert.equal(events.filter(e=>e.type==='large_fixture').length,140)
  assert.equal(new Set(events.map(e=>e.seq)).size,events.length)
  assert.deepEqual(events.map(e=>e.seq),events.map(e=>e.seq).toSorted((a,b)=>a-b))
  await assert.rejects(f.request('getSessionPage',{sessionId:first.id,cursor:{messages:-1,events:null}}),/cursor/)
})
