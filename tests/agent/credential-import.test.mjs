import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import {spawn,spawnSync} from 'node:child_process'
import {test} from 'node:test'
import {fixture,executable} from './helpers.mjs'

function source(f){const file=path.join(f.root,'.zcode','v2','provider_config.json');fs.mkdirSync(path.dirname(file),{recursive:true});return file}
function seed(f){const file=source(f);fs.writeFileSync(file,JSON.stringify({config:{providerConfigRules:{providerRules:[{providerName:'Command Code',config:{api:{type:'openai-chat-completions',baseUrl:'https://example.test/v1'},access:{apiKey:'third-party-placeholder-key'}}}]}}}));return file}

test('ordinary configuration and provider selection do not use third-party credentials',async t=>{
  const f=await fixture(t,[],{MINICODE_FAKE_SCRIPT:''});seed(f)
  assert.deepEqual((await f.request('getConfiguration')).services,[])
  await assert.rejects(f.request('setTuiProvider',{provider:'commandcode',model:'deepseek/deepseek-v4.1-flash'}),/配置|API|credential/)
  assert.ok(!JSON.stringify(f.events).includes('third-party-placeholder-key'))
})

test('explicit environment credentials continue to work without importing a file',async t=>{
  const f=await fixture(t,[],{MINICODE_FAKE_SCRIPT:'',COMMANDCODE_API_KEY:'environment-placeholder',COMMANDCODE_BASE_URL:'https://example.test/v1'})
  await f.request('setTuiProvider',{provider:'commandcode',model:'deepseek/deepseek-v4.1-flash'})
  assert.equal((await f.request('getConfiguration')).services.length,0)
})

test('disclosure and refused consent precede file access, authorized import uses secure storage',async t=>{
  const f=await fixture(t),file=source(f)
  const info=await f.request('getCredentialImportInfo',{source:'commandcode'})
  assert.equal(info.sourcePath,file);assert.match(info.dataDescription,/API Key/);assert.match(info.storageDescription,/系统|DPAPI|Keychain|Secret Service/)
  await assert.rejects(f.request('importCommandCode',{confirmed:false}),/确认|consent/)
  assert.equal(fs.existsSync(path.join(f.root,'.minicode','desktop-config.json')),false)
  seed(f)
  const result=await f.request('importCommandCode',{confirmed:true})
  assert.equal(result.services.length,1);assert.equal(result.services[0].baseUrl,'https://example.test/v1');assert.equal(result.services[0].hasApiKey,true)
  const raw=fs.readFileSync(path.join(f.root,'.minicode','desktop-config.json'),'utf8')
  assert.ok(JSON.parse(raw).services[0].credentialRef);assert.ok(!raw.includes('third-party-placeholder-key'));assert.ok(!JSON.stringify(result).includes('third-party-placeholder-key'))
  assert.ok(fs.readFileSync(file,'utf8').includes('third-party-placeholder-key'),'the source file is untouched')
})

test('unapproved discovery never opens a blocking third-party file',{skip:process.platform==='win32'},async t=>{
  const f=await fixture(t,[],{MINICODE_FAKE_SCRIPT:''}),file=source(f)
  assert.equal(spawnSync('mkfifo',[file]).status,0)
  await f.request('getCredentialImportInfo',{source:'commandcode'})
  await assert.rejects(f.request('importCommandCode',{confirmed:false}),/确认|consent/)
  await assert.rejects(f.request('setTuiProvider',{provider:'commandcode',model:'deepseek/deepseek-v4.1-flash'}),/配置|API|credential/)
})

async function cli(f,args,input){return await new Promise((resolve,reject)=>{const child=spawn(executable,args,{env:{...process.env,HOME:f.root,USERPROFILE:f.root,MINICODE_DB_PATH:f.db},windowsHide:true,stdio:['pipe','pipe','pipe']});let stdout='',stderr='';child.stdout.on('data',s=>stdout+=s);child.stderr.on('data',s=>stderr+=s);child.on('error',reject);child.on('exit',code=>resolve({code,stdout,stderr}));child.stdin.end(input)})}
test('CLI discloses source and storage before confirmation and imports only after yes',async t=>{
  const f=await fixture(t);seed(f)
  const refused=await cli(f,['config','import-commandcode'],'n\n')
  assert.equal(refused.code,0);assert.match(refused.stdout,/provider_config\.json/);assert.match(refused.stdout,/API Key/);assert.match(refused.stdout,/DPAPI|Keychain|Secret Service/)
  assert.equal((await f.request('getConfiguration')).services.length,0)
  const accepted=await cli(f,['config','import-commandcode','--yes'],'')
  assert.equal(accepted.code,0,accepted.stderr);assert.ok(!accepted.stdout.includes('third-party-placeholder-key'))
  assert.equal((await f.request('getConfiguration')).services[0].hasApiKey,true)
})

test('an authorized Command Code import does not take a different provider or leak malformed data',async t=>{
  const f=await fixture(t),file=seed(f)
  let data=JSON.parse(fs.readFileSync(file,'utf8'))
  data.config.providerConfigRules.providerRules[0].providerName='Unrelated provider'
  fs.writeFileSync(file,JSON.stringify(data))
  await assert.rejects(f.request('importCommandCode',{confirmed:true}),/Command Code/)
  fs.writeFileSync(file,'{"apiKey":"third-party-placeholder-key",broken')
  await assert.rejects(f.request('importCommandCode',{confirmed:true}),error=>!error.message.includes('third-party-placeholder-key'))
  assert.equal((await f.request('getConfiguration')).services.length,0)
})
