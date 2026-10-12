import fs from 'node:fs'
import path from 'node:path'
import {fileURLToPath} from 'node:url'
const root=path.dirname(fileURLToPath(import.meta.url))
// Host-only cases are never copied into the agent workspace.
const cases = {
  pagination_bounds:['paginate', 'assert.deepEqual(m.slicePage([0,1,2,3,4],3,2),[[4],false]);assert.deepEqual(m.slicePage([],1,3),[[],false]);assert.deepEqual(m.slicePage([1,2,3,4],1,2),[[1,2],true]);'],
  loop_off_by_one:['sum_to_n', 'for(const n of [2,3,42,100])assert.equal(m.sumToN(n),n*(n+1)/2);'],
  comparison_reversed:['in_range', 'assert.equal(m.inRange(2,2,2),true);assert.equal(m.inRange(-10,-5,3),false);assert.equal(m.inRange(3,-5,3),true);'],
  string_format:['greeting', 'assert.equal(m.displayName("Grace","Hopper"),"Grace Hopper");assert.equal(m.displayName("王","小明"),"王 小明");'],
  dict_key_typo:['user_profile', 'assert.deepEqual(m.toCard({name:"Lin",level:0,nmae:"wrong"}),{name:"Lin",level:0});'],
  division_guard:['ratio', 'assert.equal(m.percent(0,0),0);assert.equal(m.percent(-2,4),-50);assert.equal(m.percent(0,9),0);'],
  weekday_math:['weekday', 'const names=["周日","周一","周二","周三","周四","周五","周六"];for(let i=0;i<7;i++)assert.equal(m.weekdayName(new Date(Date.UTC(2026,9,11+i))),names[i]);'],
  mutable_default:['cart', 'assert.deepEqual(m.addItem(1),[1]);assert.deepEqual(m.addItem(2),[2]);const values=[];m.addItem(3,values);m.addItem(4,values);assert.deepEqual(values,[3,4]);'],
  sort_key:['leaderboard', 'const input=[{name:"a",score:0},{name:"b",score:-2},{name:"c",score:5}];assert.deepEqual(m.topScores(input),["c","a","b"]);assert.equal(input[0].name,"a");'],
  missing_return:['finder', 'assert.equal(m.firstEven([]),null);assert.equal(m.firstEven([0,2]),0);assert.equal(m.firstEven([-3,-2,8]),-2);'],
  float_rounding:['money', 'assert.equal(m.applyDiscount(12.34,5),11.72);assert.equal(m.applyDiscount(100,100),0);'],
  empty_list_bounds:['picker', 'assert.equal(m.firstOrDefault([]),null);assert.equal(m.firstOrDefault([false],true),false);assert.equal(m.firstOrDefault([""],"x"),"");'],
  regex_mismatch:['validator', 'for(const s of [""," 12345","12345x","x12345x","abcde"])assert.equal(m.isValidZip(s),false);assert.equal(m.isValidZip("00000"),true);'],
  unit_conversion:['temperature', 'assert.equal(m.celsiusToFahrenheit(37),98.6);assert.equal(m.celsiusToFahrenheit(-20),-4);'],
  dedup_logic:['dedup', 'assert.deepEqual(m.uniqueValues([true,true,false,false]),[true,false]);assert.deepEqual(m.uniqueValues(["a","a","b"]),["a","b"]);'],
  min_max_reversed:['boundary', 'assert.equal(m.clamp(2,2,2),2);assert.equal(m.clamp(-10,-5,-1),-5);assert.equal(m.clamp(-3,-5,-1),-3);'],
  fib_seed:['fib', 'assert.deepEqual(Array.from({length:8},(_,i)=>m.fib(i)),[0,1,1,2,3,5,8,13]);'],
  queue_fifo:['job_queue', 'assert.deepEqual(m.drain([]),[]);const q=[1,2,3,4];assert.deepEqual(m.drain(q),[1,2,3,4]);assert.deepEqual(q,[]);'],
  slugify_unicode:['slugify', 'assert.equal(m.slugify("  A___B  "),"a-b");assert.equal(m.slugify("世界"),"");assert.equal(m.slugify("A---B"),"a-b");'],
  cache_ttl:['cache_ttl', 'assert.equal(m.isFresh({createdAt:0,ttl:0},0),false);assert.equal(m.isFresh({createdAt:100,ttl:60},159),true);assert.equal(m.isFresh({createdAt:100,ttl:60},161),false);'],
  invoice_discount:['invoice', 'const tax=await import(pathToFileURL(path.resolve("tax.mjs")));const items=[25,75];assert.equal(m.total(items,"CA",10),96.75);assert.deepEqual(items,[25,75]);assert.equal(tax.taxAmount(200,"CA"),15);'],
  counter_retry:['counter', 'for(const x of [-99,1,200])assert.equal(m.increment(x),x+1);'],
  log_context:['service', 'assert.equal(m.finalStatus(["request=A status=started", "request=AA status=wrong", "request=A status=done"],"A"),"done");assert.equal(m.finalStatus([],"A"),null);'],
}
for(const [id,[module,checks]]of Object.entries(cases)) {
  const dir=path.join(root,'hidden_tests',id);fs.mkdirSync(dir,{recursive:true})
  fs.writeFileSync(path.join(dir,'test_hidden.test.mjs'),`import assert from 'node:assert/strict'\nimport path from 'node:path'\nimport {pathToFileURL} from 'node:url'\nimport {test} from 'node:test'\nconst m=await import(pathToFileURL(path.resolve('${module}.mjs')))\ntest('${id} hidden',async()=>{${checks}})\n`)
}
console.log(`${Object.keys(cases).length} host-side hidden test sets generated`)
