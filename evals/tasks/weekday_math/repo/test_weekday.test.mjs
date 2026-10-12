import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './weekday.mjs'
test('weekday_math',()=>{assert.equal(m.weekdayName(new Date("2026-10-12")),"周一");assert.equal(m.weekdayName(new Date("2026-10-11")),"周日");})
