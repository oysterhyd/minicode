import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('temperature.mjs')))
test('unit_conversion hidden',async()=>{assert.equal(m.celsiusToFahrenheit(37),98.6);assert.equal(m.celsiusToFahrenheit(-20),-4);})
