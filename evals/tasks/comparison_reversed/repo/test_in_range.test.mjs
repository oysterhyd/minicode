import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './in_range.mjs'
test('comparison_reversed',()=>{assert.equal(m.inRange(5,0,10),true);assert.equal(m.inRange(0,0,10),true);assert.equal(m.inRange(11,0,10),false);})
