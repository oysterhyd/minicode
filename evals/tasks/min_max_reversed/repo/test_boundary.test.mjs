import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './boundary.mjs'
test('min_max_reversed',()=>{assert.equal(m.clamp(5,0,10),5);assert.equal(m.clamp(-3,0,10),0);assert.equal(m.clamp(13,0,10),10);})
