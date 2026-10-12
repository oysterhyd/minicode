import fs from 'node:fs'
import path from 'node:path'
import {createRequire} from 'node:module'
const {transform} = createRequire(new URL('../tui/package.json', import.meta.url))('esbuild')
const runtime=fs.realpathSync(process.argv[2]),root=fs.realpathSync(path.join(runtime,'tui/node_modules'))
const within=(parent,file)=>{const relative=path.relative(parent,file);return relative!==''&&!relative.startsWith('..'+path.sep)&&!path.isAbsolute(relative)}
if(!within(runtime,root))throw Error('TUI dependencies must stay inside release runtime')
let count=0,size=0,compacted=0,saved=0
async function prune(dir){
  for(const e of fs.readdirSync(dir,{withFileTypes:true})){
    const p=path.join(dir,e.name)
    if(e.isSymbolicLink())throw Error('unexpected dependency link')
    if(e.isDirectory()){await prune(p);continue}
    if(!e.isFile())continue
    if(!within(runtime,fs.realpathSync(p)))throw Error('file outside release runtime')
    if(/\.(?:d\.ts|d\.cts|d\.mts|map|js\.flow)$/.test(e.name)){
      size+=fs.statSync(p).size;fs.unlinkSync(p);count++;continue
    }
    if(/\.(?:js|mjs|cjs)$/.test(e.name)){
      const source=fs.readFileSync(p,'utf8')
      // Preserve module paths, environment branches, function/class names and
      // license comments. No bundling or property mangling is performed.
      const {code}=await transform(source,{loader:'js',target:'node22',minify:true,keepNames:true,legalComments:'inline',charset:'utf8'})
      const reduction=Buffer.byteLength(source)-Buffer.byteLength(code)
      if(reduction>0){fs.writeFileSync(p,code);saved+=reduction;compacted++}
    }
  }
}
await prune(root)
await prune(path.join(runtime,'tui/dist'))
console.log(`Removed ${count} TUI metadata files (${size} bytes); compacted ${compacted} JS files (${saved} bytes saved); runtime paths and licenses retained.`)
