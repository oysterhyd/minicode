import fs from 'node:fs'
import {fileURLToPath} from 'node:url'
const file = fileURLToPath(new URL('log_context/repo/trace.log', import.meta.url))
const lines = Array.from({length:1600}, (_, index) => {
  const status = index===100?'request=R-17 status=started':index===900?'request=R-17 status=retrying':index===1510?'request=R-17 status=completed':`request=R-${100+index%90} status=ok`
  return `2026-09-23T12:${String(Math.floor(index/60)).padStart(2,'0')}:${String(index%60).padStart(2,'0')}Z ${status} payload=${'x'.repeat(64)}\n`
})
fs.writeFileSync(file,lines.join(''))
console.log(`${file}: ${fs.statSync(file).size} bytes`)
