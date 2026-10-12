import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './dedup.mjs'
test('dedup_logic',()=>{assert.deepEqual(m.uniqueValues([1,2,1,3,2]),[1,2,3]);assert.deepEqual(m.uniqueValues([1,"1",1]),[1,"1"]);})
