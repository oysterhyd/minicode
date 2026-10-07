import { Box, Text } from 'ink'
import { MarkdownView } from '../markdown.js'

export function AssistantMarkdown({ text, lead = true }: { text: string; lead?: boolean }) {
  return (
    <Box>
      {lead ? <Text>● </Text> : <Text>  </Text>}
      <Box flexGrow={1} flexShrink={1} minWidth={0}>
        <MarkdownView text={text} />
      </Box>
    </Box>
  )
}
