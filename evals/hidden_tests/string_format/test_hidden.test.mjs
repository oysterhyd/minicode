import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('greeting.mjs')))
test('string_format hidden',async()=>{assert.equal(m.displayName("Grace","Hopper"),"Grace Hopper");assert.equal(m.displayName("王","小明"),"王 小明");})
