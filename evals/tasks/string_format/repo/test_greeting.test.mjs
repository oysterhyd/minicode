import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './greeting.mjs'
test('string_format',()=>{assert.equal(m.displayName("Ada","Lovelace"),"Ada Lovelace");assert.equal(m.displayName("张","三"),"张 三");})
