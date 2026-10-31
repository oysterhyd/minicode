import fs from 'node:fs'
import path from 'node:path'
import {fileURLToPath} from 'node:url'

const documents = ['plan.md', 'docs/optimization-design.md', 'docs/pi-alignment-plan.md']
const codePath = /^(?:agent\/|tui\/src\/|desktop\/native\/|tests\/agent\/|evals\/)/

export function checkDocument(root, document, text) {
  const active = text.replace(/<!-- historical-paths:start -->[\s\S]*?<!-- historical-paths:end -->/g, '')
  const errors = new Set()
  for (const match of active.matchAll(/src\/minicode[^\s`\])<>]*/g)) {
    errors.add(`${document}: obsolete source path ${match[0]}`)
  }
  const references = [
    ...[...active.matchAll(/\[[^\]]*\]\(([^)]+)\)/g)].map(m => ({raw:m[1], link:true})),
    ...[...active.matchAll(/`([^`\n]+)`/g)].map(m => ({raw:m[1], link:false})),
  ]
  for (const {raw, link} of references) {
    const reference = raw.replace(/#.*$/, '').replace(/:\d+(?:-\d+)?$/, '')
    if (/^[a-z]+:\/\//i.test(reference) || /[\s*{}]/.test(reference)) continue
    const target = path.resolve(root, link ? path.dirname(document) : '', reference)
    const relative = path.relative(root, target).replaceAll('\\', '/')
    if (codePath.test(relative) && !fs.existsSync(target)) errors.add(`${document}: missing code path ${raw}`)
  }
  return [...errors]
}
export function checkDocuments(root, files = documents) {
  return files.flatMap(file => checkDocument(root, file, fs.readFileSync(path.join(root, file), 'utf8')))
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const errors = checkDocuments(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..'))
  if (errors.length) { console.error(errors.join('\n')); process.exitCode = 1 }
  else console.log('Documentation code paths verified')
}
