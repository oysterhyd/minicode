import fs from 'node:fs'
import path from 'node:path'
import {fileURLToPath} from 'node:url'
import {spawnSync} from 'node:child_process'

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'..')
const runtime=fs.realpathSync(process.argv[2])
const scratch=fs.mkdtempSync(path.join(root,'output','tui-runtime-'))
// Run the existing UI and bridge regressions against the exact compacted JS
// and production dependencies. Only the test renderer comes from development.
fs.cpSync(path.join(runtime,'tui'),scratch,{recursive:true})
fs.cpSync(path.join(root,'tui/node_modules/ink-testing-library'),path.join(scratch,'node_modules/ink-testing-library'),{recursive:true})
fs.mkdirSync(path.join(scratch,'tests'))
const tests=[]
for(const name of fs.readdirSync(path.join(root,'tui/tests'))){
  if(!/\.tsx?$/.test(name))continue
  const source=fs.readFileSync(path.join(root,'tui/tests',name),'utf8').replaceAll('../src/','../dist/')
  fs.writeFileSync(path.join(scratch,'tests',name),source)
  if(/\.test\.tsx?$/.test(name))tests.push(path.join(scratch,'tests',name))
}
const result=spawnSync(process.execPath,[path.join(root,'tui/node_modules/tsx/dist/cli.mjs'),'--test',...tests],{
  cwd:root,stdio:'inherit',windowsHide:true,env:{...process.env,MINICODE_AGENT:path.join(runtime,'minicode.exe'),MINICODE_TUI_BRIDGE_TEST:'1'},
})
if(result.error)throw result.error
if(result.status!==0)throw Error(`Compacted TUI regression suite failed (${result.status})`)
console.log('Compacted TUI full regression suite passed')
