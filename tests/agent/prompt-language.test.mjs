import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import path from 'node:path'
import {projectRoot} from './helpers.mjs'
test('A2 both platform prompts follow the user language and explicit output preference',()=>{
 for(const platform of ['posix','windows']) {
  const text=fs.readFileSync(path.join(projectRoot,`agent/assets/system-${platform}.txt`),'utf8')
  assert.ok(!text.includes('用简洁的中文总结'))
  assert.match(text,/用户.*语言/)
  assert.match(text,/明确指定/)
 }
})
