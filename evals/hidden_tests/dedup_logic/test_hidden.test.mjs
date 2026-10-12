import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('dedup.mjs')))
test('dedup_logic hidden',async()=>{assert.deepEqual(m.uniqueValues([true,true,false,false]),[true,false]);assert.deepEqual(m.uniqueValues(["a","a","b"]),["a","b"]);})
