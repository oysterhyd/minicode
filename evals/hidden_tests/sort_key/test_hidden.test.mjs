import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('leaderboard.mjs')))
test('sort_key hidden',async()=>{const input=[{name:"a",score:0},{name:"b",score:-2},{name:"c",score:5}];assert.deepEqual(m.topScores(input),["c","a","b"]);assert.equal(input[0].name,"a");})
