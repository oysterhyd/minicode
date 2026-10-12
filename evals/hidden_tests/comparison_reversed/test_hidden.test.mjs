import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('in_range.mjs')))
test('comparison_reversed hidden',async()=>{assert.equal(m.inRange(2,2,2),true);assert.equal(m.inRange(-10,-5,3),false);assert.equal(m.inRange(3,-5,3),true);})
