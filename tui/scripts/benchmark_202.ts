import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {pathToFileURL} from 'node:url'
import {performance} from 'node:perf_hooks'

const [root, output] = process.argv.slice(2)
const {applyDesktopEvent,emptyFeed} = await import(pathToFileURL(path.resolve(root,'tui/src/feed.ts')).href)
const settled = Array.from({length:30000},(_,i)=>({id:String(i),kind:'assistant',text:'completed'}))
const state={...emptyFeed(),items:[...settled,{id:'live',kind:'tool',name:'read',pending:true}],busy:true}
const cases={
  finish_30k:{event:'run_done',result:{exit_reason:'completed'}},
  append_tool_30k:{event:'agent_event',item:{type:'tool_call_start',seq:1,timestamp:'2026-01-01T00:00:00Z',data:{call_id:'new',name:'read',arguments:{path:'fixture'}}}},
}
const results:Record<string,unknown>={}
for(const [name,event] of Object.entries(cases)){
  const runs:number[]=[]
  for(let sample=-2;sample<7;sample++){
    const start=performance.now()
    for(let i=0;i<100;i++){
      const next=applyDesktopEvent(state,event)
      assert.equal(next.items.length,name.startsWith('finish')?state.items.length:state.items.length+1)
      assert.equal(next.items[0].text,'completed')
      assert.equal(state.items.at(-1)!.pending,true)
    }
    if(sample>=0)runs.push((performance.now()-start)/100)
  }
  results[name]={median_ms:[...runs].sort((a,b)=>a-b)[3],runs_ms:runs,calls_per_sample:100}
}
fs.writeFileSync(output,JSON.stringify({node:process.version,root:path.resolve(root),measurements:results},null,2)+'\n')
