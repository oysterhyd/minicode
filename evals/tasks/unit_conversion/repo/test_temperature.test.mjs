import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './temperature.mjs'
test('unit_conversion',()=>{assert.equal(m.celsiusToFahrenheit(0),32);assert.equal(m.celsiusToFahrenheit(100),212);assert.equal(m.celsiusToFahrenheit(-40),-40);})
