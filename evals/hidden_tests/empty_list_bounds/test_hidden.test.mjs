import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('picker.mjs')))
test('empty_list_bounds hidden',async()=>{assert.equal(m.firstOrDefault([]),null);assert.equal(m.firstOrDefault([false],true),false);assert.equal(m.firstOrDefault([""],"x"),"");})
