import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('service.mjs')))
test('log_context hidden',async()=>{assert.equal(m.finalStatus(["request=A status=started", "request=AA status=wrong", "request=A status=done"],"A"),"done");assert.equal(m.finalStatus([],"A"),null);})
