import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('ratio.mjs')))
test('division_guard hidden',async()=>{assert.equal(m.percent(0,0),0);assert.equal(m.percent(-2,4),-50);assert.equal(m.percent(0,9),0);})
