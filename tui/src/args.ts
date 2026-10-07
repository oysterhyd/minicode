export type LaunchOptions = {
  workspace: string
  provider?: string
  model?: string
  script?: string
  maxRounds?: number
  maxTokens?: number
  maxSeconds?: number
  yes?: boolean
  db?: string
  acceptance?: string
  help?: boolean
}

export function parseArgs(argv: string[]): LaunchOptions {
  const options: LaunchOptions = { workspace: '.' }
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index]
    const take = () => {
      const value = argv[++index]
      if (value === undefined || !value.trim() || value.startsWith('--')) throw new Error(`缺少 ${arg} 的值`)
      return value
    }
    switch (arg) {
      case '--workspace':
        options.workspace = take()
        break
      case '--provider':
        options.provider = take()
        break
      case '--model':
        options.model = take()
        break
      case '--script':
        options.script = take()
        break
      case '--max-rounds':
        options.maxRounds = Number(take())
        break
      case '--max-tokens':
        options.maxTokens = Number(take())
        break
      case '--max-seconds':
        options.maxSeconds = Number(take())
        break
      case '--yes':
      case '-y':
        options.yes = true
        break
      case '--db':
        options.db = take()
        break
      case '--acceptance':
        options.acceptance = take()
        break
      case '--help':
      case '-h':
        options.help = true
        break
      default:
        throw new Error(`未知参数：${arg}`)
    }
  }
  if (options.provider && !['auto', 'fake', 'anthropic', 'commandcode'].includes(options.provider.toLowerCase())) {
    throw new Error(`未知 provider：${options.provider}`)
  }
  if (options.provider) options.provider = options.provider.toLowerCase()
  for (const [name, value, integer, nonnegative] of [
    ['--max-rounds', options.maxRounds, true, true],
    ['--max-tokens', options.maxTokens, true, false],
    ['--max-seconds', options.maxSeconds, false, true],
  ] as const) {
    if (value !== undefined && (!Number.isFinite(value) || (integer && !Number.isSafeInteger(value)) || (nonnegative && value < 0))) {
      throw new Error(`${name} 的值不合法`)
    }
  }
  if (options.script && options.provider && !['auto', 'fake'].includes(options.provider)) {
    throw new Error('--script 只能与 auto 或 fake provider 一起使用')
  }
  return options
}

export const HELP_TEXT = `minicode tui — Claude Code 风格的终端界面

用法:
  minicode tui [选项]

选项:
  --workspace DIR       工作区目录（默认当前目录）
  --provider NAME       auto | fake | anthropic | commandcode
  --model NAME          模型名称；--provider fake 时使用离线演示
  --script FILE         FakeProvider 脚本 JSON
  --max-rounds N        每次执行的轮次切片（0 = 不设）
  --max-tokens N        会话累计 token 上限（0 = 不限制）
  --max-seconds N       每次执行的时长切片（0 = 不限）
  --yes, -y             自动允许全部工具调用
  --db PATH             会话数据库路径
  --acceptance FILE     验收配置 YAML
`
