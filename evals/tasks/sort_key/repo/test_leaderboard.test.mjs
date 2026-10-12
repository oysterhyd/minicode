import assert from 'node:assert/strict'
import {test} from 'node:test'
import * as m from './leaderboard.mjs'
test('sort_key',()=>{assert.deepEqual(m.topScores([{name:"low",score:1},{name:"high",score:9}]),["high","low"]);})
