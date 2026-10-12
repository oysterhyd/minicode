import fs from 'node:fs'
import path from 'node:path'
const runtime=fs.realpathSync(process.argv[2]),root=fs.realpathSync(path.join(runtime,'tui/node_modules'))
const within=(parent,file)=>{const relative=path.relative(parent,file);return relative!==''&&!relative.startsWith('..'+path.sep)&&!path.isAbsolute(relative)}
if(!within(runtime,root))throw Error('TUI dependencies must stay inside release runtime')
let count=0,size=0
function prune(dir){for(const e of fs.readdirSync(dir,{withFileTypes:true})){const p=path.join(dir,e.name);if(e.isSymbolicLink())throw Error('unexpected dependency link');if(e.isDirectory()){prune(p);continue}if(e.isFile()&&/\.(?:d\.ts|d\.cts|d\.mts|map)$/.test(e.name)){if(!within(root,fs.realpathSync(p)))throw Error('metadata outside TUI dependencies');size+=fs.statSync(p).size;fs.unlinkSync(p);count++}}}
prune(root)
console.log(`Removed ${count} TUI declaration/source-map files (${size} bytes); runtime and licenses retained.`)
