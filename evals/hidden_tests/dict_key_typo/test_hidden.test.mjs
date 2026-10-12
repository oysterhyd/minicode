import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('user_profile.mjs')))
test('dict_key_typo hidden',async()=>{assert.deepEqual(m.toCard({name:"Lin",level:0,nmae:"wrong"}),{name:"Lin",level:0});})
