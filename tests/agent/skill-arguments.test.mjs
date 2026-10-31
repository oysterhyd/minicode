import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import path from 'node:path'
import {fixture} from './helpers.mjs'
test('L14 skill subcommands normalize whitespace and validate argument counts',async t=>{
 const f=await fixture(t)
 const dir=path.join(f.workspace,'.minicode/skills/review');fs.mkdirSync(dir,{recursive:true});fs.writeFileSync(path.join(dir,'SKILL.md'),'---\nname: review\ndescription: review\n---\nReview changes.')
 for(const arg of ['off','on','off review extra','review extra']) await assert.rejects(f.request('runSlash',{text:`/skill ${arg}`,workspace:f.workspace}),/用法/)
 await f.request('runSlash',{text:'/skill\ton\t review',workspace:f.workspace})
 assert.equal((await f.request('getCapabilities',{workspace:f.workspace})).skills.find(s=>s.name==='review').active,true)
 await f.request('runSlash',{text:'/skill  off \t review  ',workspace:f.workspace})
 assert.equal((await f.request('getCapabilities',{workspace:f.workspace})).skills.find(s=>s.name==='review').active,false)
})
