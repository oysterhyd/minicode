import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './money.mjs'
test('float_rounding',()=>{assert.equal(m.applyDiscount(19.99,10),17.99);assert.equal(m.applyDiscount(9.5,0),9.5);})
