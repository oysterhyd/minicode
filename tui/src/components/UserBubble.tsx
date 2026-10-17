import { Box, Text } from 'ink'
import { terminalText } from '../format.js'
import { theme } from '../terminal.js'

// Claude Code echoes the prompt as "> text" on a soft grey band.
export function UserBubble({ text }: { text: string }) {
  const lines = terminalText(text).split('\n')
  return (
    <Box marginBottom={1} width="100%" backgroundColor={theme.userBg}>
      <Text color={theme.subtle}>{'> '}</Text>
      <Box flexDirection="column" flexGrow={1} flexShrink={1} minWidth={0}>
        {lines.map((line, index) => <Text key={index}>{line || ' '}</Text>)}
      </Box>
    </Box>
  )
}
