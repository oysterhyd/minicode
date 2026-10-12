import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('boundary.mjs')))
test('min_max_reversed hidden',async()=>{assert.equal(m.clamp(2,2,2),2);assert.equal(m.clamp(-10,-5,-1),-5);assert.equal(m.clamp(-3,-5,-1),-3);})
