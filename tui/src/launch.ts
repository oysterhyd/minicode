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
    if (fs.existsSync(path.join(current, 'Cargo.toml')) && fs.existsSync(path.join(current, 'agent', 'main.rs'))) {
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
  const env: NodeJS.ProcessEnv = { ...options.environment }
  const resourcesPath = options.resourcesPath || env.MINICODE_RESOURCES
  if (resourcesPath) {
    const runtime = path.join(resourcesPath, 'runtime')
    const pathKey = Object.keys(env).find(key => key.toLowerCase() === 'path') || 'PATH'
    env[pathKey] = [path.join(runtime, 'git', 'cmd'), path.join(runtime, 'node'), env[pathKey] || ''].join(path.delimiter)
    return {
      file: path.join(runtime, process.platform === 'win32' ? 'minicode.exe' : 'minicode'),
      args: ['bridge'],
      cwd: options.userData || process.cwd(),
      env,
    }
  }
  const here = options.projectRoot || projectRootFrom(fileURLToPath(import.meta.url)) || process.cwd()
  const name = process.platform === 'win32' ? 'minicode.exe' : 'minicode'
  const release = path.join(here, 'target', 'release', name)
  return {
    file: env.MINICODE_AGENT || (fs.existsSync(release) ? release : path.join(here, 'target', 'debug', name)),
    args: ['bridge'],
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
