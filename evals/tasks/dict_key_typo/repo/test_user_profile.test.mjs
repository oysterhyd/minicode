import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './user_profile.mjs'
test('dict_key_typo',()=>{assert.deepEqual(m.toCard({name:"Ada",level:3}),{name:"Ada",level:3});})
