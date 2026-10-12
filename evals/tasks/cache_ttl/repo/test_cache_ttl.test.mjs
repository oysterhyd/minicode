import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './cache_ttl.mjs'
test('cache_ttl',()=>{assert.equal(m.isFresh({createdAt:10,ttl:5},12),true);assert.equal(m.isFresh({createdAt:10,ttl:5},15),false);})
