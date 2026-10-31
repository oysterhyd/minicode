import assert from 'node:assert/strict'
import test from 'node:test'
import { ArtifactPages } from '../src/artifact-pages.js'
import { FakeBridge } from './fake-bridge.js'

test('TU5 artifact pages replace content and use Unicode character offsets', async () => {
  const client = new FakeBridge()
  client.handlers.set('readArtifact', p => ({ text: p.offset ? 'last' : '😀一', hasMore: !p.offset, total: p.offset ? 6 : null }))
  const pages = new ArtifactPages(client, 's', 'a')
  assert.equal((await pages.load(0)).text, '😀一')
  assert.equal((await pages.next()).text, 'last')
  assert.equal(client.requests.at(-1)?.params.offset, 2)
  assert.equal((await pages.previous()).text, '😀一')
  client.handlers.set('readArtifact', () => { throw Error('artifact missing') })
  await assert.rejects(pages.next(), /artifact missing/)
  assert.equal(pages.page.text, '😀一')
})
