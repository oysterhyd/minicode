import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('weekday.mjs')))
test('weekday_math hidden',async()=>{const names=["周日","周一","周二","周三","周四","周五","周六"];for(let i=0;i<7;i++)assert.equal(m.weekdayName(new Date(Date.UTC(2026,9,11+i))),names[i]);})
