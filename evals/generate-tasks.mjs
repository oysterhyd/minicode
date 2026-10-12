import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

// Portable equivalents of the original twenty repository bug classes.
// All fixtures use Node.js, which is already shipped for the Ink frontend.
export const tasks = [
  ['pagination_bounds','paginate','export function slicePage(items,page=1,pageSize=10){if(page<1)throw Error("page must be >= 1");const start=(page-1)*pageSize;const end=start+pageSize+1;return [items.slice(start,end),end<items.length]}','const end=start+pageSize+1','const end=start+pageSize', 'assert.deepEqual(m.slicePage(Array.from({length:25},(_,i)=>i)),[Array.from({length:10},(_,i)=>i),true]);assert.deepEqual(m.slicePage([1,2,3],2,2),[[3],false]);assert.throws(()=>m.slicePage([],0));'],
  ['loop_off_by_one','sum_to_n','export function sumToN(n){let total=0;for(let i=1;i<n;i++)total+=i;return total}','i<n;','i<=n;', 'assert.equal(m.sumToN(1),1);assert.equal(m.sumToN(10),55);assert.equal(m.sumToN(0),0);'],
  ['comparison_reversed','in_range','export function inRange(v,low,high){return v<low||v>high}','v<low||v>high','low<=v&&v<=high', 'assert.equal(m.inRange(5,0,10),true);assert.equal(m.inRange(0,0,10),true);assert.equal(m.inRange(11,0,10),false);'],
  ['string_format','greeting','export function displayName(first,last){return `${last} ${first}`}','`${last} ${first}`','`${first} ${last}`', 'assert.equal(m.displayName("Ada","Lovelace"),"Ada Lovelace");assert.equal(m.displayName("张","三"),"张 三");'],
  ['dict_key_typo','user_profile','export function toCard(user){return {name:user.nmae,level:user.level}}','user.nmae','user.name', 'assert.deepEqual(m.toCard({name:"Ada",level:3}),{name:"Ada",level:3});'],
  ['division_guard','ratio','export function percent(part,whole){return part/whole*100}','return part/whole*100','return whole===0?0:part/whole*100', 'assert.equal(m.percent(1,4),25);assert.equal(m.percent(1,0),0);'],
  ['weekday_math','weekday','const names=["周一","周二","周三","周四","周五","周六","周日"];export function weekdayName(day){return names[(day.getUTCDay()+6)%7+1]}','(day.getUTCDay()+6)%7+1','(day.getUTCDay()+6)%7', 'assert.equal(m.weekdayName(new Date("2026-10-12")),"周一");assert.equal(m.weekdayName(new Date("2026-10-11")),"周日");'],
  ['mutable_default','cart','const shared=[];export function addItem(item,cart=shared){cart.push(item);return cart}','cart=shared','cart=[]', 'assert.deepEqual(m.addItem("a"),["a"]);assert.deepEqual(m.addItem("b"),["b"]);const cart=["x"];assert.equal(m.addItem("y",cart),cart);'],
  ['sort_key','leaderboard','export function topScores(players){return [...players].sort((a,b)=>a.score-b.score).map(p=>p.name)}','a.score-b.score','b.score-a.score', 'assert.deepEqual(m.topScores([{name:"low",score:1},{name:"high",score:9}]),["high","low"]);'],
  ['missing_return','finder','export function firstEven(numbers){let result=null;for(const n of numbers){if(n%2===0){result=n;break}}}','break}}','break}}return result;', 'assert.equal(m.firstEven([1,4,6]),4);assert.equal(m.firstEven([1,3]),null);'],
  ['float_rounding','money','export function applyDiscount(price,percent){return Math.round(price*(1-percent/100))}','Math.round(price*(1-percent/100))','Math.round(price*(1-percent/100)*100)/100', 'assert.equal(m.applyDiscount(19.99,10),17.99);assert.equal(m.applyDiscount(9.5,0),9.5);'],
  ['empty_list_bounds','picker','export function firstOrDefault(items,fallback=null){return items[0]}','return items[0]','return items.length?items[0]:fallback', 'assert.equal(m.firstOrDefault([],"none"),"none");assert.equal(m.firstOrDefault([0]),0);'],
  ['regex_mismatch','validator','export function isValidZip(code){return /[0-9]{5}/.test(code)}','/[0-9]{5}/','/^[0-9]{5}$/', 'assert.equal(m.isValidZip("12345"),true);assert.equal(m.isValidZip("123456"),false);assert.equal(m.isValidZip("x12345"),false);'],
  ['unit_conversion','temperature','export function celsiusToFahrenheit(c){return c*9/5+30}','c*9/5+30','c*9/5+32', 'assert.equal(m.celsiusToFahrenheit(0),32);assert.equal(m.celsiusToFahrenheit(100),212);assert.equal(m.celsiusToFahrenheit(-40),-40);'],
  ['dedup_logic','dedup','export function uniqueValues(values){const seen=new Set();const result=[];for(const value of values){if(seen.has(value))continue;result.push(value);seen.add(String(value))}return result}','seen.add(String(value))','seen.add(value)', 'assert.deepEqual(m.uniqueValues([1,2,1,3,2]),[1,2,3]);assert.deepEqual(m.uniqueValues([1,"1",1]),[1,"1"]);'],
  ['min_max_reversed','boundary','export function clamp(value,low,high){return Math.max(high,Math.min(low,value))}','Math.max(high,Math.min(low,value))','Math.min(high,Math.max(low,value))', 'assert.equal(m.clamp(5,0,10),5);assert.equal(m.clamp(-3,0,10),0);assert.equal(m.clamp(13,0,10),10);'],
  ['fib_seed','fib','export function fib(n){let a=0,b=0;for(let i=0;i<n;i++){[a,b]=[b,a+b]}return a}','let a=0,b=0','let a=0,b=1', 'assert.equal(m.fib(0),0);assert.equal(m.fib(1),1);assert.equal(m.fib(10),55);'],
  ['queue_fifo','job_queue','export function drain(jobs){const order=[];while(jobs.length)order.push(jobs.pop());return order}','jobs.pop()','jobs.shift()', 'const jobs=["a","b","c"];assert.deepEqual(m.drain(jobs),["a","b","c"]);assert.deepEqual(jobs,[]);'],
  ['slugify_unicode','slugify','export function slugify(text){return text.toLowerCase().replace(/[^a-z0-9-]/g,"-")}','replace(/[^a-z0-9-]/g,"-")','replace(/[^a-z0-9]+/g,"-").replace(/^-+|-+$/g,"")', 'assert.equal(m.slugify("Hello  世界 test"),"hello-test");assert.equal(m.slugify("--ABC--"),"abc");'],
  ['cache_ttl','cache_ttl','export function isFresh(entry,now){return now-entry.createdAt>entry.ttl}','now-entry.createdAt>entry.ttl','now-entry.createdAt<entry.ttl', 'assert.equal(m.isFresh({createdAt:10,ttl:5},12),true);assert.equal(m.isFresh({createdAt:10,ttl:5},15),false);'],
]
export function generate(root) {
  for (const [id,module,source,oldText,newText,checks] of tasks) {
    const dir=path.join(root,id), repo=path.join(dir,'repo'), filename=`${module}.mjs`, testFile=`test_${module}.test.mjs`
    fs.mkdirSync(repo,{recursive:true})
    fs.writeFileSync(path.join(repo,filename),source+'\n')
    fs.writeFileSync(path.join(repo,testFile),`import assert from 'node:assert/strict'\nimport {test} from 'node:test'\nimport * as m from './${filename}'\ntest('${id}',()=>{${checks}})\n`)
    const command=`node --test ${testFile}`
    const task={id,title:id,prompt:`阅读 ${filename}，修复现有缺陷，通过 ${command}。只允许修改 ${filename}，不得修改测试。`,allowed_paths:[filename],protected_paths:[testFile],items:[{id:'tests-pass',type:'command',command},{id:'tests-unchanged',type:'protected',path:testFile}],budget:{max_rounds:10,max_tokens:80000,max_seconds:120},max_fix_attempts:3,script:'script.json'}
    // JSON is a valid YAML mapping and avoids a second dependency for fixtures.
    fs.writeFileSync(path.join(dir,'task.yaml'),JSON.stringify(task,null,2)+'\n')
    fs.writeFileSync(path.join(dir,'script.json'),JSON.stringify({turns:[{tool_calls:[{name:'read',arguments:{path:filename}}]},{tool_calls:[{name:'edit',arguments:{path:filename,old_text:oldText,new_text:newText}}]},{tool_calls:[{name:'bash',arguments:{command}}]},{text:'已修复并通过测试。'}]},null,2)+'\n')
  }
}
if (process.argv[1]===fileURLToPath(import.meta.url)) {
  const root=path.join(path.dirname(fileURLToPath(import.meta.url)),'tasks')
  generate(root)
  if (process.argv.includes('--verify')) {
    const os=await import('node:os')
    for (const [id,module,,oldText,newText] of tasks) {
      const temp=fs.mkdtempSync(path.join(os.tmpdir(),'minicode-eval-verify-'))
      try {
        fs.cpSync(path.join(root,id,'repo'),temp,{recursive:true})
        const run=()=>spawnSync(process.execPath,['--test',`test_${module}.test.mjs`],{cwd:temp,encoding:'utf8'})
        if (run().status===0)throw Error(`${id}: broken fixture unexpectedly passes`)
        const file=path.join(temp,`${module}.mjs`)
        fs.writeFileSync(file,fs.readFileSync(file,'utf8').replace(oldText,newText))
        const result=run();if(result.status!==0)throw Error(`${id}: fixed fixture fails\n${result.stdout}`)
      } finally {fs.rmSync(temp,{recursive:true,force:true})}
    }
    console.log('20 evaluation fixtures: red -> green verified')
  }
}
