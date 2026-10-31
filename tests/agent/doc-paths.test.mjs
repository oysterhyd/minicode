import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {test} from 'node:test'
import {checkDocument, checkDocuments} from '../../scripts/check-doc-paths.mjs'
import {projectRoot} from './helpers.mjs'

test('E2 active documentation points to existing source paths', () => {
  assert.deepEqual(checkDocuments(projectRoot), [])
})
test('E2 checker rejects stale links and inline paths; history requires an explicit block', t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'minicode-doc-path-'))
  t.after(() => fs.rmSync(root, {recursive:true, force:true}))
  fs.mkdirSync(path.join(root, 'agent'))
  fs.writeFileSync(path.join(root, 'agent/runtime.rs'), '')
  assert.deepEqual(checkDocument(root, 'docs/example.md', '[Runtime](../agent/runtime.rs#new) `agent/runtime.rs:12`'), [])
  assert.equal(checkDocument(root, 'docs/example.md', '[old](../src/minicode/cli.py)').length, 1)
  assert.equal(checkDocument(root, 'docs/example.md', '`agent/missing.rs`').length, 1)
  assert.deepEqual(checkDocument(root, 'plan.md', '<!-- historical-paths:start -->\n`src/minicode/old.py`\n<!-- historical-paths:end -->'), [])
})
