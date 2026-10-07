import assert from 'node:assert/strict'
import test from 'node:test'
import { parseArgs } from '../src/args.js'

test('parseArgs maps CLI flags', () => {
  const options = parseArgs([
    '--workspace', 'D:\\repo',
    '--provider', 'fake',
    '--yes',
    '--max-rounds', '8',
    '--script', 'script.json',
  ])
  assert.equal(options.workspace, 'D:\\repo')
  assert.equal(options.provider, 'fake')
  assert.equal(options.yes, true)
  assert.equal(options.maxRounds, 8)
  assert.equal(options.script, 'script.json')
})

test('parseArgs rejects unknown flags', () => {
  assert.throws(() => parseArgs(['--nope']), /未知参数/)
})
