import fs from 'node:fs'
const [main,scenarios]=process.argv.slice(2)
const core=JSON.parse(fs.readFileSync(`${main}/results.json`,'utf8'))
if(core.run_count!==60||core.passed!==60||core.results.some(r=>r.hidden_pass!==true))throw Error('20-task B0/B1/B2 or hidden acceptance regression')
const extra=JSON.parse(fs.readFileSync(`${scenarios}/results.json`,'utf8'))
if(extra.run_count!==9||extra.passed!==7)throw Error('scenario result count regression')
for(const r of extra.results){const expected=r.task!=='counter_retry'||r.baseline==='b2';if(r.passed!==expected)throw Error(`unexpected ${r.task}/${r.baseline} result`)}
console.log('60 task runs and 9 scenarios preserve expected acceptance results')
