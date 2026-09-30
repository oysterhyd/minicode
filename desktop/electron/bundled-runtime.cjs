const fs = require('node:fs')
const path = require('node:path')

function bridgeLaunch({ packaged, resourcesPath, projectRoot, userData, environment = process.env }) {
  const env = { ...environment, PYTHONIOENCODING: 'utf-8', PYTHONUTF8: '1' }
  if (packaged) {
    const runtime = path.join(resourcesPath, 'runtime')
    const python = path.join(runtime, 'python')
    const pathKey = Object.keys(env).find(key => key.toLowerCase() === 'path') || 'PATH'
    env[pathKey] = [python, path.join(python, 'Scripts'), path.join(runtime, 'git', 'cmd'), env[pathKey] || ''].join(path.delimiter)
    delete env.PYTHONHOME
    delete env.PYTHONPATH
    return { file: path.join(python, 'python.exe'),
      args: ['-I', '-X', 'utf8', '-u', path.join(runtime, 'bridge.py')], cwd: userData, env }
  }
  const venv = process.platform === 'win32' ? path.join(projectRoot, '.venv', 'Scripts', 'python.exe') : path.join(projectRoot, '.venv', 'bin', 'python')
  env.PYTHONPATH = path.join(projectRoot, 'src')
  return { file: fs.existsSync(venv) ? venv : process.platform === 'win32' ? 'python' : 'python3',
    args: ['-u', path.join(projectRoot, 'desktop', 'bridge.py')], cwd: projectRoot, env }
}

module.exports = { bridgeLaunch }
