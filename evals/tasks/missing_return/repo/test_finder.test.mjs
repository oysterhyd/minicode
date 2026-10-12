import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './finder.mjs'
test('missing_return',()=>{assert.equal(m.firstEven([1,4,6]),4);assert.equal(m.firstEven([1,3]),null);})
