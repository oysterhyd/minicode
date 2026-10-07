import { Box, Text } from 'ink'
import { displayWidth, terminalText } from '../format.js'
import { ACCENT, useTerminalSize } from '../terminal.js'

const LOGO = [
  ' ███╗   ███╗ ██╗ ███╗   ██╗ ██╗  ██████╗  ██████╗  ██████╗  ███████╗',
  ' ████╗ ████║ ██║ ████╗  ██║ ██║ ██╔════╝ ██╔═══██╗ ██╔══██╗ ██╔════╝',
  ' ██╔████╔██║ ██║ ██╔██╗ ██║ ██║ ██║      ██║   ██║ ██║  ██║ █████╗',
  ' ██║╚██╔╝██║ ██║ ██║╚██╗██║ ██║ ██║      ██║   ██║ ██║  ██║ ██╔══╝',
  ' ██║ ╚═╝ ██║ ██║ ██║ ╚████║ ██║ ╚██████╗ ╚██████╔╝ ██████╔╝ ███████╗',
  ' ╚═╝     ╚═╝ ╚═╝ ╚═╝  ╚═══╝ ╚═╝  ╚═════╝  ╚═════╝  ╚═════╝  ╚══════╝',
]

const LETTERS: Record<string, string[]> = {
  M: ['█   █', '██ ██', '█ █ █', '█   █', '█   █'],
  I: ['███', ' █ ', ' █ ', ' █ ', '███'],
  N: ['█  █', '██ █', '█ ██', '█  █', '█  █'],
  C: [' ███', '█   ', '█   ', '█   ', ' ███'],
  O: [' ██ ', '█  █', '█  █', '█  █', ' ██ '],
  D: ['███ ', '█  █', '█  █', '█  █', '███ '],
  E: ['████', '█   ', '███ ', '█   ', '████'],
}

function wordmark(word: string): string[] {
  return Array.from({ length: 5 }, (_, row) => [...word].map(letter => LETTERS[letter][row]).join(' ').trimEnd())
}

const COMPACT_LOGO = wordmark('MINICODE')
const STACKED_LOGO = [...wordmark('MINI'), '', ...wordmark('CODE')]

function fits(lines: string[], columns: number): boolean {
  return lines.every(line => displayWidth(line) <= columns)
}

export type BannerProps = { version: string; workspace: string; model: string; permissionMode?: string }

export function Banner({ version, workspace, model }: BannerProps) {
  const { columns } = useTerminalSize()
  const width = Math.max(0, columns - 2)
  const logo = fits(LOGO, width) ? LOGO
    : fits(COMPACT_LOGO, width) ? COMPACT_LOGO
      : fits(STACKED_LOGO, width) ? STACKED_LOGO : []
  return (
    <Box flexDirection="column" marginY={1} paddingX={1}>
      {logo.length ? (
        <Box flexDirection="column" marginBottom={1}>
          {logo.map((line, index) => <Text key={index} color={ACCENT} bold>{line || ' '}</Text>)}
        </Box>
      ) : null}
      <Text><Text color={ACCENT} bold>✻ MiniCode</Text><Text dimColor>  v{version}</Text></Text>
      <Text dimColor>{terminalText(model || '选择模型后开始')} · {terminalText(workspace)}</Text>
      <Text dimColor>/help 查看命令  ·  /resume 恢复会话</Text>
    </Box>
  )
}
