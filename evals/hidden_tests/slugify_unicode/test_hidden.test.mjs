import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('slugify.mjs')))
test('slugify_unicode hidden',async()=>{assert.equal(m.slugify("  A___B  "),"a-b");assert.equal(m.slugify("世界"),"");assert.equal(m.slugify("A---B"),"a-b");})
