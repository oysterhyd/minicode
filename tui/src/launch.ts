import { spawn, type ChildProcess } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

export type LaunchSpec = {
  file: string
  args: string[]
  cwd: string
  env: NodeJS.ProcessEnv
}

export function projectRootFrom(start: string): string | null {
  let current = start.startsWith('file:') ? path.dirname(fileURLToPath(start)) : start
  for (let depth = 0; depth < 8; depth++) {
    if (fs.existsSync(path.join(current, 'desktop', 'bridge.py')) && fs.existsSync(path.join(current, 'src', 'minicode'))) {
      return current
    }
    const parent = path.dirname(current)
    if (parent === current) break
    current = parent
  }
  return null
}

export function bridgeLaunch(options: {
  resourcesPath?: string
  projectRoot?: string
  userData?: string
  environment?: NodeJS.ProcessEnv
}): LaunchSpec {
  const env: NodeJS.ProcessEnv = { ...options.environment, PYTHONIOENCODING: 'utf-8', PYTHONUTF8: '1' }
  const resourcesPath = options.resourcesPath || env.MINICODE_RESOURCES
  if (resourcesPath) {
    const runtime = path.join(resourcesPath, 'runtime')
    const python = path.join(runtime, 'python')
    const pathKey = Object.keys(env).find(key => key.toLowerCase() === 'path') || 'PATH'
    env[pathKey] = [python, path.join(python, 'Scripts'), path.join(runtime, 'git', 'cmd'), env[pathKey] || ''].join(path.delimiter)
    delete env.PYTHONHOME
    delete env.PYTHONPATH
    return {
      file: path.join(python, process.platform === 'win32' ? 'python.exe' : 'python'),
      args: ['-I', '-X', 'utf8', '-u', path.join(runtime, 'bridge.py')],
      cwd: options.userData || process.cwd(),
      env,
    }
  }
  const here = options.projectRoot || projectRootFrom(fileURLToPath(import.meta.url)) || process.cwd()
  const venv = process.platform === 'win32'
    ? path.join(here, '.venv', 'Scripts', 'python.exe')
    : path.join(here, '.venv', 'bin', 'python')
  env.PYTHONPATH = path.join(here, 'src')
  return {
    file: env.MINICODE_PYTHON || (fs.existsSync(venv) ? venv : process.platform === 'win32' ? 'python' : 'python3'),
    args: ['-u', path.join(here, 'desktop', 'bridge.py')],
    cwd: here,
    env,
  }
}

export function spawnBridge(options: {
  resourcesPath?: string
  projectRoot?: string
  userData?: string
  extraEnv?: NodeJS.ProcessEnv
} = {}): ChildProcess {
  const launch = bridgeLaunch({
    resourcesPath: options.resourcesPath,
    projectRoot: options.projectRoot,
    userData: options.userData,
    environment: { ...process.env, ...options.extraEnv },
  })
  return spawn(launch.file, launch.args, {
    cwd: launch.cwd,
    env: launch.env,
    stdio: ['pipe', 'pipe', 'pipe'],
    windowsHide: true,
    detached: process.platform !== 'win32',
  })
}
