import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './slugify.mjs'
test('slugify_unicode',()=>{assert.equal(m.slugify("Hello  世界 test"),"hello-test");assert.equal(m.slugify("--ABC--"),"abc");})
