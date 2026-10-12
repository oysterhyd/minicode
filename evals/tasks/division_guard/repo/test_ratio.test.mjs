import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './ratio.mjs'
test('division_guard',()=>{assert.equal(m.percent(1,4),25);assert.equal(m.percent(1,0),0);})
