import fs from 'node:fs'
import path from 'node:path'
import {spawnSync} from 'node:child_process'
const result=spawnSync('cargo',['metadata','--locked','--format-version','1','--filter-platform','x86_64-pc-windows-msvc'],{encoding:'utf8',maxBuffer:32*1024*1024})
if(result.status!==0)throw Error('Cargo metadata failed')
const metadata=JSON.parse(result.stdout),root=path.resolve(process.argv[2]);fs.mkdirSync(root,{recursive:true})
const records=[]
for(const pkg of metadata.packages){if(pkg.name==='minicode')continue;records.push({name:pkg.name,version:pkg.version,license:pkg.license,repository:pkg.repository});const source=path.dirname(pkg.manifest_path),target=path.join(root,`${pkg.name}-${pkg.version}`);fs.mkdirSync(target,{recursive:true});for(const entry of fs.readdirSync(source,{withFileTypes:true}))if(entry.isFile()&&/^(LICENSE|COPYING|NOTICE)/i.test(entry.name))fs.copyFileSync(path.join(source,entry.name),path.join(target,entry.name));if(pkg.license_file){const filename=path.resolve(source,pkg.license_file);if(fs.existsSync(filename))fs.copyFileSync(filename,path.join(target,path.basename(filename)));}}
fs.writeFileSync(path.join(root,'dependencies.json'),JSON.stringify(records,null,2)+'\n')
console.log(`Rust dependency notices: ${records.length} packages`)
