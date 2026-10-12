import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './fib.mjs'
test('fib_seed',()=>{assert.equal(m.fib(0),0);assert.equal(m.fib(1),1);assert.equal(m.fib(10),55);})
