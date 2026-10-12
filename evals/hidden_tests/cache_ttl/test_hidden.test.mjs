import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('cache_ttl.mjs')))
test('cache_ttl hidden',async()=>{assert.equal(m.isFresh({createdAt:0,ttl:0},0),false);assert.equal(m.isFresh({createdAt:100,ttl:60},159),true);assert.equal(m.isFresh({createdAt:100,ttl:60},161),false);})
