import { Box, Text } from 'ink'
import { terminalText } from '../format.js'

export function UserBubble({ text }: { text: string }) {
  const lines = terminalText(text).split('\n')
  return (
    <Box flexDirection="column" marginY={1} width="100%">
      {lines.map((line, index) => (
        <Box key={index} width="100%">
          <Text bold>
            {index === 0 ? '❯ ' : '  '}
            {line || ' '}
          </Text>
        </Box>
      ))}
    </Box>
  )
}
