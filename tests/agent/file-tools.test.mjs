import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {test} from 'node:test'
import {DatabaseSync} from 'node:sqlite'
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

test('normalized batch edits reach the bridge and failures preserve the file',async t=>{
  const f=await fixture(t,[{tool_calls:[
    {name:'read',arguments:{path:'edit.txt'}},
    {name:'edit',arguments:{path:'edit.txt',edits:[{old_text:'log("café");',new_text:'updated'},{old_text:'tail',new_text:'end'}]}},
    {name:'edit',arguments:{path:'edit.txt',edits:[{old_text:'updated',new_text:'partial'},{old_text:'missing',new_text:'bad'}]}}
  ]},{text:'done'}])
  fs.writeFileSync(path.join(f.workspace,'edit.txt'),'log(“café”);\ntail\n')
  await f.request('setPermissionMode',{mode:'bypass'})
  const result=await f.run()
  const calls=result.detail.events.filter(e=>e.type==='tool_call_result')
  assert.equal(calls[1].data.success,true,JSON.stringify(calls))
  assert.equal(calls[2].data.success,false)
  assert.match(calls[2].data.error,/Nearest candidate lines/)
  assert.equal(fs.readFileSync(path.join(f.workspace,'edit.txt'),'utf8'),'updated\nend\n')
})

test('artifact character defaults and line/search/tail views work through tool results',async t=>{
  const f=await fixture(t,[{text:'ready'},{tool_calls:[
    {id:'characters',name:'read_artifact',arguments:{artifact_id:'seed'}},
    {id:'lines',name:'read_artifact',arguments:{artifact_id:'seed',start_line:2,end_line:3}},
    {id:'search',name:'read_artifact',arguments:{artifact_id:'seed',pattern:'needle'}},
    {id:'tail',name:'read_artifact',arguments:{artifact_id:'seed',tail_lines:2}}
  ]},{text:'done'}])
  const first=await f.run()
  const dir=path.join(f.root,'artifacts',first.id);fs.mkdirSync(dir,{recursive:true})
  fs.writeFileSync(path.join(dir,'seed.txt'),'alpha\nneedle中\nomega\n'+'tail\n'.repeat(1200))
  const db=new DatabaseSync(f.db)
  try {
    db.exec('PRAGMA busy_timeout=5000')
    db.prepare('INSERT INTO artifacts(session_id,artifact_id,kind,relpath,created_at) VALUES(?,?,?,?,?)').run(first.id,'seed','command_output','seed.txt',new Date().toISOString())
  } finally {db.close()}
  const result=await f.run({sessionId:first.id})
  const calls=Object.fromEntries(result.detail.events.filter(e=>e.type==='tool_call_result').map(e=>[e.data.call_id,e.data]))
  assert.ok(Object.values(calls).every(c=>c.success),JSON.stringify(calls))
  assert.equal([...calls.characters.output_detail.split('\n...[更多内容')[0]].length,4000)
  assert.equal(calls.lines.output_detail,'2\tneedle中\n3\tomega')
  assert.equal(calls.search.output_detail,'2\tneedle中')
  assert.equal(calls.tail.output_detail,'1202\ttail\n1203\ttail')
})
