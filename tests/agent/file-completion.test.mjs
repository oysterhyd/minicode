import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import path from 'node:path'
import {fixture} from './helpers.mjs'
test('T-UI6 shared file completion preserves spaces and Unicode and honors ignores', async t => {
  const f = await fixture(t)
  fs.writeFileSync(path.join(f.workspace, '文件 name.txt'), 'ok')
  fs.writeFileSync(path.join(f.workspace, '.gitignore'), 'secret.txt\n')
  fs.writeFileSync(path.join(f.workspace, 'secret.txt'), 'ignored')
  const files = await f.request('listFiles', {workspace:f.workspace, query:'文件'})
  assert.deepEqual(files, ['文件 name.txt'])
  assert.ok(!(await f.request('listFiles', {workspace:f.workspace})).includes('secret.txt'))
})
