import { Box, Text } from 'ink'
import { marked, type Token, type Tokens } from 'marked'
import type { ReactNode } from 'react'
import { displayWidth, terminalText } from './format.js'
import { useTerminalSize } from './terminal.js'

const MATH_PREFIX = '§math§'

function closeOpenFence(source: string): string {
  let fence: string | null = null
  for (const line of source.split('\n')) {
    const marker = line.match(/^\s*(`{3,}|~{3,})/)?.[1]
    if (!marker) continue
    if (fence === null) fence = marker
    else if (marker[0] === fence[0] && marker.length >= fence.length && /^\s*[`~]+\s*$/.test(line)) fence = null
  }
  return fence ? `${source}\n${fence}` : source
}

function preprocessMath(source: string): string {
  const convert = (text: string) => text.replace(
    /(`+)[\s\S]*?\1|\$\$([\s\S]+?)\$\$|\\\[([\s\S]+?)\\\]|\\\((.+?)\\\)|\$([^$\n]+?)\$/g,
    (all, code: string | undefined, dollars: string | undefined, brackets: string | undefined, parens: string | undefined, single: string | undefined) => {
      if (code) return all
      const display = dollars ?? brackets
      if (display !== undefined) return `\n\`\`\`math\n${display.trim()}\n\`\`\`\n`
      const body = parens ?? single ?? ''
      if (single && /^\s*\d/.test(single)) return all // prices such as "$5 and $10"
      return `\`${MATH_PREFIX}${body.trim()}\``
    },
  )
  const result: string[] = []
  let prose: string[] = []
  let fence: string | undefined
  const flush = () => { if (prose.length) result.push(convert(prose.join('\n'))); prose = [] }
  for (const line of source.split('\n')) {
    const marker = line.match(/^\s*(?:>\s*)*(`{3,}|~{3,})/)?.[1]
    if (fence) {
      result.push(line)
      if (marker && marker[0] === fence[0] && marker.length >= fence.length
        && /^\s*(?:>\s*)*[`~]+\s*$/.test(line)) fence = undefined
    } else if (marker || /^(?: {4}|\t)/.test(line)) {
      flush()
      result.push(line)
      if (marker) fence = marker
    } else prose.push(line)
  }
  flush()
  return result.join('\n')
}

function tokensToPlain(tokens: Token[] | undefined): string {
  if (!tokens?.length) return ''
  return tokens.map(token => {
    if ('tokens' in token && Array.isArray(token.tokens)) return tokensToPlain(token.tokens)
    if ('items' in token && Array.isArray(token.items)) return tokensToPlain(token.items as Token[])
    if (token.type === 'br') return '\n'
    if ('text' in token && typeof token.text === 'string') return token.text
    return ''
  }).join('')
}

function padCell(text: string, width: number, align?: string): string {
  const extra = Math.max(0, width - displayWidth(text))
  if (align === 'right') return `${' '.repeat(extra)}${text}`
  if (align === 'center') {
    const left = Math.floor(extra / 2)
    return `${' '.repeat(left)}${text}${' '.repeat(extra - left)}`
  }
  return `${text}${' '.repeat(extra)}`
}

function cellText(cell: string | Token | Tokens.TableCell | undefined): string {
  if (cell == null) return ''
  if (typeof cell === 'string') return cell
  if ('tokens' in cell && Array.isArray(cell.tokens)) return tokensToPlain(cell.tokens)
  if ('text' in cell && typeof cell.text === 'string') return cell.text
  return ''
}

function inline(tokens: Token[] | undefined): ReactNode {
  if (!tokens?.length) return null
  return tokens.map((token, index) => {
    const key = `${token.type}-${index}`
    if (token.type === 'text') return <Text key={key}>{token.tokens?.length ? inline(token.tokens) : token.text}</Text>
    if (token.type === 'strong') return <Text key={key} bold>{inline(token.tokens)}</Text>
    if (token.type === 'em') return <Text key={key} italic>{inline(token.tokens)}</Text>
    if (token.type === 'del') return <Text key={key} strikethrough dimColor>{inline(token.tokens)}</Text>
    if (token.type === 'codespan') {
      const value = token.text.startsWith(MATH_PREFIX) ? token.text.slice(MATH_PREFIX.length) : token.text
      return <Text key={key} color="cyan" italic={token.text.startsWith(MATH_PREFIX)}>{value}</Text>
    }
    if (token.type === 'link') {
      const label = tokensToPlain(token.tokens) || token.text || token.href
      return (
        <Text key={key} color="cyan" underline>
          {label}
          {token.href && token.href !== label ? ` (${token.href})` : ''}
        </Text>
      )
    }
    if (token.type === 'br') return <Text key={key}>{'\n'}</Text>
    if (token.type === 'escape') return <Text key={key}>{token.text}</Text>
    if (token.type === 'image') return <Text key={key} dimColor>{token.text || token.href}</Text>
    if ('tokens' in token && Array.isArray(token.tokens)) return <Text key={key}>{inline(token.tokens)}</Text>
    if ('text' in token && typeof token.text === 'string') return <Text key={key}>{token.text}</Text>
    return null
  })
}

function heading(token: Tokens.Heading) {
  return (
    <Box marginTop={token.depth === 1 ? 1 : 0} marginBottom={1}>
      <Text bold>{inline(token.tokens)}</Text>
    </Box>
  )
}

function codeBlock(token: Tokens.Code) {
  const math = token.lang === 'math'
  const lines = token.text.replace(/\n$/, '').split('\n')
  return (
    <Box flexDirection="column" marginY={1} paddingX={1} borderStyle="round" borderColor={math ? 'cyan' : 'gray'}>
      {token.lang ? <Text dimColor italic={math}>{math ? '公式' : token.lang}</Text> : null}
      {lines.map((line, index) => (
        <Text key={index} color={math ? 'cyan' : undefined} italic={math}>{line || ' '}</Text>
      ))}
    </Box>
  )
}

function table(token: Tokens.Table, columns: number) {
  const header = token.header.map(cell => cellText(cell))
  const rows = token.rows.map(row => row.map(cell => cellText(cell)))
  const widths = header.map((cell, index) => Math.max(
    displayWidth(cell),
    rows.reduce((max, row) => Math.max(max, displayWidth(row[index] || '')), 0),
  ))
  if (widths.reduce((sum, width) => sum + width, 0) + (widths.length - 1) * 3 > columns - 4) {
    return (
      <Box flexDirection="column" marginY={1}>
        {rows.map((row, index) => (
          <Box key={index} flexDirection="column" marginBottom={1}>
            {row.map((cell, cellIndex) => <Text key={cellIndex}><Text bold>{header[cellIndex]}: </Text>{cell}</Text>)}
          </Box>
        ))}
      </Box>
    )
  }
  const align = token.align || []
  const format = (cells: string[]) => cells
    .map((cell, index) => padCell(cell, widths[index] || 0, align[index] || undefined))
    .join(' │ ')
  return (
    <Box flexDirection="column" marginY={1} paddingX={1}>
      <Text bold>{format(header)}</Text>
      <Text dimColor>{widths.map(width => '─'.repeat(Math.max(1, width))).join('─┼─')}</Text>
      {rows.map((row, index) => <Text key={index}>{format(row)}</Text>)}
    </Box>
  )
}

function list(token: Tokens.List, columns: number) {
  return (
    <Box flexDirection="column" marginBottom={1}>
      {token.items.map((item, index) => {
        const mark = item.task ? `${item.checked ? '☑' : '☐'} ` : token.ordered ? `${Number(token.start || 1) + index}. ` : '• '
        return (
          <Box key={index}>
            <Box width={displayWidth(mark)} flexShrink={0}><Text>{mark}</Text></Box>
            <Box flexDirection="column" flexGrow={1} flexShrink={1} minWidth={0}>
              {item.tokens.map((child, childIndex) => block(child, childIndex, columns - displayWidth(mark)))}
            </Box>
          </Box>
        )
      })}
    </Box>
  )
}

function block(token: Token, index: number, columns: number): ReactNode {
  if (token.type === 'space') return null
  if (token.type === 'text') return <Text key={index}>{inline('tokens' in token ? token.tokens : [token])}</Text>
  if (token.type === 'heading') return <Box key={index}>{heading(token as Tokens.Heading)}</Box>
  if (token.type === 'paragraph') {
    return (
      <Box key={index} marginBottom={1}>
        <Text wrap="wrap">{inline((token as Tokens.Paragraph).tokens)}</Text>
      </Box>
    )
  }
  if (token.type === 'code') return <Box key={index}>{codeBlock(token as Tokens.Code)}</Box>
  if (token.type === 'blockquote') {
    const quote = token as Tokens.Blockquote
    return (
      <Box key={index} flexDirection="column" marginBottom={1} paddingLeft={1} borderStyle="single" borderColor="gray" borderTop={false} borderBottom={false} borderRight={false}>
        {(quote.tokens || []).map((child, childIndex) => (
          <Box key={childIndex} flexDirection="column">{block(child, childIndex, columns - 2)}</Box>
        ))}
      </Box>
    )
  }
  if (token.type === 'list') return <Box key={index}>{list(token as Tokens.List, columns)}</Box>
  if (token.type === 'table') return <Box key={index}>{table(token as Tokens.Table, columns)}</Box>
  if (token.type === 'hr') {
    return (
      <Box key={index} marginY={1}>
        <Text dimColor>{'─'.repeat(24)}</Text>
      </Box>
    )
  }
  if (token.type === 'html') return <Text key={index} dimColor>{(token as Tokens.HTML).text}</Text>
  if ('tokens' in token && Array.isArray(token.tokens)) {
    return (
      <Box key={index} marginBottom={1}>
        <Text wrap="wrap">{inline(token.tokens)}</Text>
      </Box>
    )
  }
  if ('text' in token && typeof token.text === 'string') {
    return (
      <Box key={index} marginBottom={1}>
        <Text wrap="wrap">{token.text}</Text>
      </Box>
    )
  }
  return null
}

/** Split markdown at blank lines outside fenced code so finished blocks can freeze while streaming. */
export function splitBlocks(text: string): string[] {
  const blocks: string[] = []
  let current: string[] = []
  let fence: string | null = null
  for (const line of text.split('\n')) {
    const marker = line.match(/^\s*(`{3,}|~{3,})/)?.[1]
    if (marker && fence === null) fence = marker
    else if (marker && fence && marker[0] === fence[0] && marker.length >= fence.length && /^\s*[`~]+\s*$/.test(line)) fence = null
    if (!fence && line.trim() === '' && current.length && !/^\s*([-*+]|\d+[.)])\s/.test(current[current.length - 1] || '')) {
      blocks.push(current.join('\n'))
      current = []
      continue
    }
    current.push(line)
  }
  if (current.length) blocks.push(current.join('\n'))
  return blocks
}

export function MarkdownView({ text }: { text: string }) {
  const { columns } = useTerminalSize()
  const source = closeOpenFence(preprocessMath(terminalText(text, 4)))
  const tokens = marked.lexer(source, { gfm: true, breaks: false })
  return <Box flexDirection="column" flexGrow={1} flexShrink={1} minWidth={0}>{tokens.map((token, index) => block(token, index, columns - 2))}</Box>
}
