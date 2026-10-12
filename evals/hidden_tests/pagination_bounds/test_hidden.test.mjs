import assert from 'node:assert/strict'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {test} from 'node:test'
const m=await import(pathToFileURL(path.resolve('paginate.mjs')))
test('pagination_bounds hidden',async()=>{assert.deepEqual(m.slicePage([0,1,2,3,4],3,2),[[4],false]);assert.deepEqual(m.slicePage([],1,3),[[],false]);assert.deepEqual(m.slicePage([1,2,3,4],1,2),[[1,2],true]);})
