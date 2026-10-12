import assert from 'node:assert/strict'
import {test} from 'node:test'
import {total} from "./invoice.mjs";import {taxAmount} from "./tax.mjs";test("discount and tax",()=>{assert.equal(total([40,60],"CA",20),86);assert.equal(taxAmount(200,"CA"),15);assert.equal(total([10],"NONE",1),9)})
