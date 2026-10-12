import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('sum_to_n.mjs')))
test('loop_off_by_one hidden',async()=>{for(const n of [2,3,42,100])assert.equal(m.sumToN(n),n*(n+1)/2);})
