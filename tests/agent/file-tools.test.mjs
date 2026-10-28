import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {fixture} from './helpers.mjs'

test('read/edit preserve Chinese legacy encodings and UTF-8 BOM through the bridge',async t=>{
  for (const encoding of ['gbk','utf16le','utf8-bom']) {
    const encode = text => encoding === 'gbk'
      ? Buffer.from(text === '中文\n' ? 'd6d0cec40a' : 'cac0bde70a','hex')
      : Buffer.concat([encoding === 'utf16le' ? Buffer.from([255,254]) : Buffer.from([239,187,191]),Buffer.from(text,encoding === 'utf16le' ? 'utf16le' : 'utf8')])
    const f = await fixture(t,[{tool_calls:[{name:'read',arguments:{path:'encoded.txt'}},{name:'edit',arguments:{path:'encoded.txt',old_text:'中文',new_text:'世界'}},{name:'read',arguments:{path:'encoded.txt'}}]},{text:'done'}])
    fs.writeFileSync(path.join(f.workspace,'encoded.txt'),encode('中文\n'))
    await f.request('setPermissionMode',{mode:'bypass'})
    const result = await f.run()
    const calls = result.detail.events.filter(e=>e.type==='tool_call_result')
    assert.equal(calls.length,3)
    assert.ok(calls.every(e=>e.data.success),JSON.stringify(calls))
    assert.equal(calls[0].data.output_detail,'1\t中文')
    assert.equal(calls[2].data.output_detail,'1\t世界')
    assert.deepEqual(fs.readFileSync(path.join(f.workspace,'encoded.txt')),encode('世界\n'))
  }
})

test('grep honors ignore files and exposes an explicit override',async t=>{
  const f=await fixture(t,[{tool_calls:[
    {name:'grep',arguments:{pattern:'needle'}},
    {name:'grep',arguments:{pattern:'needle',include_ignored:true}},
    {name:'grep',arguments:{pattern:'needle(?=$)'}}
  ]},{text:'done'}])
  fs.writeFileSync(path.join(f.workspace,'.gitignore'),'target/\n')
  fs.writeFileSync(path.join(f.workspace,'.ignore'),'private.txt\n')
  fs.mkdirSync(path.join(f.workspace,'target'))
  for(const file of ['source.txt','private.txt','target/noise.txt']) fs.writeFileSync(path.join(f.workspace,file),'needle\n')
  const result=await f.run()
  const calls=result.detail.events.filter(e=>e.type==='tool_call_result').sort((a,b)=>a.data.call_id.localeCompare(b.data.call_id))
  assert.equal(calls[0].data.output_detail,'source.txt:1: needle',JSON.stringify(calls))
  assert.match(calls[1].data.output_detail,/target\/noise.txt/)
  assert.equal(calls[2].data.success,false)
  assert.match(calls[2].data.error,/not supported|look-around/)
})
