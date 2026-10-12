import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('cart.mjs')))
test('mutable_default hidden',async()=>{assert.deepEqual(m.addItem(1),[1]);assert.deepEqual(m.addItem(2),[2]);const values=[];m.addItem(3,values);m.addItem(4,values);assert.deepEqual(values,[3,4]);})
