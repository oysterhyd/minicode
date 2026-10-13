import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { spawn, spawnSync } from 'node:child_process'
const args=process.argv.slice(2),root=path.resolve(args.find(a=>!a.startsWith('--'))||'.')
const source=args.includes('--source'),history=args.includes('--history')
const secrets=new Set()
for(const [key,value]of Object.entries(process.env))if(/(API_?KEY|PRIVATE_?KEY|SIGNING_?KEY|TOKEN|SECRET|PASSWORD)$/i.test(key)&&value.length>=8)secrets.add(value)
function visit(value){if(!value||typeof value!=='object')return;for(const [key,item]of Object.entries(value)){if(/api.?key|token|secret|password|authorization/i.test(key)&&typeof item==='string'&&item.length>=8)secrets.add(item);visit(item)}}
for(const file of ['.minicode/desktop-config.json','.zcode/v2/provider_config.json']){const p=path.join(os.homedir(),file);if(fs.existsSync(p))visit(JSON.parse(fs.readFileSync(p,'utf8').replace(/^\uFEFF/,'')))}
const bytes=[...secrets].map(s=>Buffer.from(s)),overlap=Math.max(1,...bytes.map(b=>b.length))-1
const problems=new Set()
function scan(fd,length,label){let tail=Buffer.alloc(0),left=length,buffer=Buffer.alloc(4*1024*1024);while(left>0){let n=fs.readSync(fd,buffer,0,Math.min(buffer.length,left),null);if(!n)throw Error('unexpected EOF');left-=n;const data=Buffer.concat([tail,buffer.subarray(0,n)]);if(bytes.some(s=>data.includes(s)))problems.add(`local credential found: ${label}`);if(label.endsWith('.pem')&&data.includes(Buffer.from('PRIVATE KEY-----')))problems.add(`private key: ${label}`);tail=overlap?data.subarray(Math.max(0,data.length-overlap)):Buffer.alloc(0)}}
function walk(dir,result=[]){for(const entry of fs.readdirSync(dir,{withFileTypes:true})){let p=path.join(dir,entry.name);if(entry.isDirectory())walk(p,result);else if(entry.isFile())result.push(p);else if(entry.isSymbolicLink())throw Error(`unexpected publication link: ${p}`)}return result}
const files=source?spawnSync('git',['ls-files','-z','--cached','--others','--exclude-standard'],{cwd:root,encoding:'utf8',maxBuffer:16*1024*1024}).stdout.split('\0').filter(Boolean).map(p=>path.join(root,p)):walk(root)
let count=0
for(const file of files){if(!fs.existsSync(file)||!fs.statSync(file).isFile())continue;const rel=path.relative(root,file),name=path.basename(file).toLowerCase();if(['desktop-config.json','provider_config.json','credentials.json','.env'].includes(name)||(name.startsWith('.env.')&&name!=='.env.example')||/\.(?:key|db|db-journal)$/.test(name)||rel.split(path.sep).some(s=>['.minicode','.zcode','.codex','.claude','.git'].includes(s.toLowerCase())))problems.add(`personal file: ${rel}`);const fd=fs.openSync(file,'r');try{scan(fd,fs.fstatSync(fd).size,rel)}finally{fs.closeSync(fd)}count++}
if(history&&bytes.length){
  const child=spawn('git',['cat-file','--batch','--batch-all-objects'],{cwd:root,stdio:['ignore','pipe','pipe'],windowsHide:true})
  let header=Buffer.alloc(0),remaining=0,label='',kind='',tail=Buffer.alloc(0),blobs=0,separator=false,errors=''
  child.stderr.setEncoding('utf8');child.stderr.on('data',data=>errors+=data)
  for await(const chunk of child.stdout){let offset=0;while(offset<chunk.length){
    if(separator){if(chunk[offset++]!==10)throw Error('invalid Git batch separator');separator=false;continue}
    if(remaining){const n=Math.min(remaining,chunk.length-offset),part=chunk.subarray(offset,offset+n);if(kind==='blob'){const data=Buffer.concat([tail,part]);if(bytes.some(secret=>data.includes(secret)))problems.add('local credential in Git blob: '+label);tail=overlap?Buffer.from(data.subarray(Math.max(0,data.length-overlap))):Buffer.alloc(0)}offset+=n;remaining-=n;if(!remaining)separator=true;continue}
    const newline=chunk.indexOf(10,offset);if(newline<0){header=Buffer.concat([header,chunk.subarray(offset)]);if(header.length>1024)throw Error('invalid Git batch header');break}
    header=Buffer.concat([header,chunk.subarray(offset,newline)]);const fields=header.toString('utf8').split(' ');if(fields.length!==3)throw Error('invalid Git object header');[label,kind]=fields;remaining=Number(fields[2]);if(!Number.isSafeInteger(remaining)||remaining<0)throw Error('invalid Git object size');header=Buffer.alloc(0);tail=Buffer.alloc(0);offset=newline+1;if(kind==='blob')blobs++;if(remaining===0)separator=true
  }}
  const status=await new Promise(resolve=>child.exitCode===null?child.once('close',resolve):resolve(child.exitCode));if(status!==0||header.length||remaining)throw Error('Git history audit failed: '+errors)
  console.log('Git history audit passed: '+blobs+' blobs inspected')
}
if(problems.size)throw Error('Publication audit failed:\n'+[...problems].sort().join('\n'))
console.log(`Publication audit passed: ${count} files; no personal files or known local credentials.`)
