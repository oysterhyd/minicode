import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('job_queue.mjs')))
test('queue_fifo hidden',async()=>{assert.deepEqual(m.drain([]),[]);const q=[1,2,3,4];assert.deepEqual(m.drain(q),[1,2,3,4]);assert.deepEqual(q,[]);})
