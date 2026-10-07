#!/usr/bin/env node
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import { render } from 'ink'
import { createElement } from 'react'
import { HELP_TEXT, parseArgs } from './args.js'
import { App } from './App.js'
import { BridgeClient } from './bridge.js'
import { spawnBridge } from './launch.js'
import { enterTerminalScreen } from './terminal.js'

export async function main(argv = process.argv.slice(2)): Promise<number> {
  let options
  try {
    options = parseArgs(argv)
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : error}\n`)
    return 1
  }
  if (options.help) {
    process.stdout.write(HELP_TEXT)
    return 0
  }
  if (!process.stdin.isTTY || !process.stdout.isTTY) {
    process.stderr.write('minicode tui 需要交互式终端。\n')
    return 1
  }
  options.workspace = path.resolve(options.workspace)
  if (!fs.existsSync(options.workspace) || !fs.statSync(options.workspace).isDirectory()) {
    process.stderr.write(`工作区不存在或不是目录：${options.workspace}\n`)
    return 1
  }
  if (options.acceptance) options.acceptance = path.resolve(options.acceptance)
  const extraEnv: NodeJS.ProcessEnv = {}
  if (options.db) extraEnv.MINICODE_DB_PATH = path.resolve(options.db)
  if (options.script) extraEnv.MINICODE_FAKE_SCRIPT = path.resolve(options.script)
  const child = spawnBridge({ extraEnv })
  const client = new BridgeClient(child)
  const restoreTerminal = enterTerminalScreen()
  try {
    const instance = render(createElement(App, { client, options, version: '1.1.0' }), {
      exitOnCtrlC: false,
    })
    await instance.waitUntilExit()
    return 0
  } finally {
    restoreTerminal()
    await client.close()
  }
}

const entry = process.argv[1] && path.resolve(process.argv[1])
if (entry && path.resolve(fileURLToPath(import.meta.url)) === entry) {
  main().then(code => {
    if (code) process.exit(code)
  }, error => {
    process.stderr.write(`${error instanceof Error ? error.message : error}\n`)
    process.exit(1)
  })
}
