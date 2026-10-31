import fs from 'node:fs'
import path from 'node:path'
import {fileURLToPath} from 'node:url'

export function compareReports(directories) {
  const reports = directories.map(directory => ({directory, config:JSON.parse(fs.readFileSync(path.join(directory,'config.json'))), stats:JSON.parse(fs.readFileSync(path.join(directory,'statistics.json')))}))
  const first = reports[0]?.config
  if (!first) throw Error('Provide at least one evaluation output directory')
  const comparability = config => JSON.stringify([config.fixtures,config.temperature,config.seed,config.prices,config.concurrency,config.schedule])
  const rows = reports.flatMap(({directory,config,stats}) => stats.groups.map(group => ({directory,version:stats.version,agent_sha256:stats.agent_sha256,comparable:comparability(config)===comparability(first),...group})))
  return {schema_version:1, rows}
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(JSON.stringify(compareReports(process.argv.slice(2)), null, 2))
}
