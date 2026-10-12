import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('money.mjs')))
test('float_rounding hidden',async()=>{assert.equal(m.applyDiscount(12.34,5),11.72);assert.equal(m.applyDiscount(100,100),0);})
