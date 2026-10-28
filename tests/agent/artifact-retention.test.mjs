import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {DatabaseSync} from 'node:sqlite'
import {fixture} from './helpers.mjs'

for (const mode of ['quota', 'lru', 'age', 'disabled']) {
  test(`output artifact retention: ${mode}`, async t => {
    const calls = [0,1,2].map(i => ({name:'bash', arguments:{command:i === 2
      ? 'node -e "const t=setInterval(()=>{if(require(\'fs\').existsSync(\'continue-retention\')){clearInterval(t);process.stdout.write(\'2\'.repeat(60000))}},20)"'
      : `node -e "process.stdout.write('${i}'.repeat(60000))"`}}))
    const f = await fixture(t, [{tool_calls:calls},{text:'done'}], {
      MINICODE_ARTIFACT_MAX_BYTES: mode === 'disabled' || mode === 'age' ? '0' : '150000',
      MINICODE_ARTIFACT_MAX_AGE_DAYS: mode === 'age' ? '1' : '0'
    })
    await f.request('setPermissionMode', {mode:'bypass'})
    const running = f.run()
    running.catch(() => {}) // Fixture teardown may reject a run after an assertion fails.
    const results = () => f.events.filter(e => e.event === 'agent_event' && e.item.type === 'tool_call_result')
    await f.wait(() => results().length >= 2)
    const first = results()[0].item.data.artifact_id
    if (mode === 'lru') await f.request('readArtifact', {sessionId:results()[0].sessionId, artifactId:first,offset:0,limit:1})
    if (mode === 'age') {
      const db = new DatabaseSync(f.db)
      try {
        db.exec('PRAGMA busy_timeout=5000')
        db.prepare("UPDATE artifacts SET created_at='2000-01-01T00:00:00+00:00' WHERE artifact_id=?").run(first)
      } finally { db.close() }
    }
    fs.writeFileSync(path.join(f.workspace,'continue-retention'),'go')
    const result = await running
    const ids = result.detail.events.filter(e => e.type === 'tool_call_result').map(e => e.data.artifact_id)
    const dir = path.join(f.root,'artifacts',result.id)
    const files = fs.readdirSync(dir)
    assert.equal(files.length, mode === 'disabled' ? 3 : 2)
    const evicted = mode === 'lru' ? ids[1] : ids[0]
    if (mode !== 'disabled') assert.ok(!files.includes(`${evicted}.txt`))
    assert.ok(files.includes(`${ids[2]}.txt`))
  })
}
