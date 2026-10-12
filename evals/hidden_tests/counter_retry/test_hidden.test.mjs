import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('counter.mjs')))
test('counter_retry hidden',async()=>{for(const x of [-99,1,200])assert.equal(m.increment(x),x+1);})
