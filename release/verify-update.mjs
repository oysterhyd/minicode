import fs from 'node:fs'
import path from 'node:path'
import { createHash, createPublicKey, verify } from 'node:crypto'
import { fileURLToPath } from 'node:url'
export function verifyUpdate(manifestPath,configPath) {
  const manifest=JSON.parse(fs.readFileSync(manifestPath,'utf8'))
  const config=JSON.parse(fs.readFileSync(configPath,'utf8'))
  if(manifest.version!==config.version)throw Error('update version differs from application version')
  const url=new URL(manifest.url)
  const prefix=`/${config.updates.github}/releases/download/v${config.version}/`
  if(url.protocol!=='https:'||url.hostname!=='github.com'||!url.pathname.startsWith(prefix))throw Error('update URL does not point to the configured GitHub release')
  const archive=path.join(path.dirname(manifestPath),path.basename(url.pathname))
  if(fs.statSync(archive).size!==manifest.size)throw Error('update archive has the wrong size')
  const hash=createHash('sha256'),fd=fs.openSync(archive,'r'),buf=Buffer.alloc(4*1024*1024)
  try{for(let n;(n=fs.readSync(fd,buf))>0;)hash.update(buf.subarray(0,n))}finally{fs.closeSync(fd)}
  const raw=Buffer.from(config.updates.publicKey,'base64')
  if(raw.length!==32)throw Error('invalid update public key')
  const key=createPublicKey({key:Buffer.concat([Buffer.from('302a300506032b6570032100','hex'),raw]),format:'der',type:'spki'})
  if(!verify(null,hash.digest(),key,Buffer.from(manifest.signature,'base64')))throw Error('invalid update signature')
  return archive
}
if(process.argv[1]===fileURLToPath(import.meta.url))console.log(`Signed update verified: ${path.basename(verifyUpdate(process.argv[2],process.argv[3]))}`)
