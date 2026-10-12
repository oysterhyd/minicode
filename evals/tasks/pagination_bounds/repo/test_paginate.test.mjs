import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './paginate.mjs'
test('pagination_bounds',()=>{assert.deepEqual(m.slicePage(Array.from({length:25},(_,i)=>i)),[Array.from({length:10},(_,i)=>i),true]);assert.deepEqual(m.slicePage([1,2,3],2,2),[[3],false]);assert.throws(()=>m.slicePage([],0));})
