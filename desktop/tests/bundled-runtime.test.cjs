const test = require('node:test')
const assert = require('node:assert/strict')
const path = require('node:path')
const { bridgeLaunch } = require('../electron/bundled-runtime.cjs')

test('packaged bridge uses bundled tools, an isolated interpreter and a writable cwd', () => {
  const environment = { Path: 'system-tools', PYTHONHOME: 'foreign-python', PYTHONPATH: 'foreign-modules', ANTHROPIC_API_KEY: 'test-key' }
  const launch = bridgeLaunch({ packaged: true, resourcesPath: '/installed/resources', userData: '/profile', environment })
  assert.equal(launch.file, path.join('/installed/resources/runtime/python', 'python.exe'))
  assert.equal(launch.cwd, '/profile')
  assert.deepEqual(launch.args.slice(0, 4), ['-I', '-X', 'utf8', '-u'])
  assert.equal(launch.env.PYTHONHOME, undefined)
  assert.equal(launch.env.PYTHONPATH, undefined)
  assert.equal(launch.env.ANTHROPIC_API_KEY, 'test-key')
  assert.equal(launch.env.Path.split(path.delimiter).at(-1), 'system-tools')
  assert.match(launch.env.Path, /git/)
  assert.equal(environment.PYTHONHOME, 'foreign-python')
})
