import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './sum_to_n.mjs'
test('loop_off_by_one',()=>{assert.equal(m.sumToN(1),1);assert.equal(m.sumToN(10),55);assert.equal(m.sumToN(0),0);})
