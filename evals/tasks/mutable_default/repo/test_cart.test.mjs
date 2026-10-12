import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './cart.mjs'
test('mutable_default',()=>{assert.deepEqual(m.addItem("a"),["a"]);assert.deepEqual(m.addItem("b"),["b"]);const cart=["x"];assert.equal(m.addItem("y",cart),cart);})
