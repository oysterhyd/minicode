import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './picker.mjs'
test('empty_list_bounds',()=>{assert.equal(m.firstOrDefault([],"none"),"none");assert.equal(m.firstOrDefault([0]),0);})
