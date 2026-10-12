import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('fib.mjs')))
test('fib_seed hidden',async()=>{assert.deepEqual(Array.from({length:8},(_,i)=>m.fib(i)),[0,1,1,2,3,5,8,13]);})
