import assert from 'node:assert/strict'
import {test} from 'node:test'
import {increment} from "./counter.mjs";test("increment",()=>{assert.equal(increment(0),1);assert.equal(increment(99),100)})
