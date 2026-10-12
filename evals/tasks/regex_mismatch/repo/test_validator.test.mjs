import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './validator.mjs'
test('regex_mismatch',()=>{assert.equal(m.isValidZip("12345"),true);assert.equal(m.isValidZip("123456"),false);assert.equal(m.isValidZip("x12345"),false);})
