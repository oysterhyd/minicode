import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('invoice.mjs')))
test('invoice_discount hidden',async()=>{const tax=await import(pathToFileURL(path.resolve("tax.mjs")));const items=[25,75];assert.equal(m.total(items,"CA",10),96.75);assert.deepEqual(items,[25,75]);assert.equal(tax.taxAmount(200,"CA"),15);})
