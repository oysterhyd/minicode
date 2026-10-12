import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('finder.mjs')))
test('missing_return hidden',async()=>{assert.equal(m.firstEven([]),null);assert.equal(m.firstEven([0,2]),0);assert.equal(m.firstEven([-3,-2,8]),-2);})
