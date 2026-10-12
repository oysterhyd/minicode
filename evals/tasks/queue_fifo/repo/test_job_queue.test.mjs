import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './job_queue.mjs'
test('queue_fifo',()=>{const jobs=["a","b","c"];assert.deepEqual(m.drain(jobs),["a","b","c"]);assert.deepEqual(jobs,[]);})
