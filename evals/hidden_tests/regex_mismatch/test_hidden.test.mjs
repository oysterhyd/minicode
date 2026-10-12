import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('validator.mjs')))
test('regex_mismatch hidden',async()=>{for(const s of [""," 12345","12345x","x12345x","abcde"])assert.equal(m.isValidZip(s),false);assert.equal(m.isValidZip("00000"),true);})
