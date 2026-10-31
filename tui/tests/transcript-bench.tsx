import React from 'react'
import { Box, Text, renderToString } from 'ink'
import { performance } from 'node:perf_hooks'
import { promptRows } from '../src/caret.js'
import { PasteStore } from '../src/input-history.js'

const rows = Array.from({ length: 1200 }, (_, i) => <Text key={i}>message {i}: completed transcript row</Text>)
function measure(count: number, tree: React.ReactNode) {
  const started = performance.now()
  for (let i = 0; i < count; i++) renderToString(tree, { columns: 80 })
  return (performance.now() - started) / count
}
// Same public Ink renderer, width, messages and 20 updates. The old viewport
// reconciled all rows; TranscriptLog emits sealed rows once outside that tree.
const paste = 'pasted 中文 line\n'.repeat(10000)
const reference = new PasteStore().fold(paste)
function measurePrompt(text: string) {
  const started = performance.now()
  for (let i = 0; i < 20; i++) promptRows(text, text.length, 76)
  return (performance.now() - started) / 20
}
console.log(JSON.stringify({ rows: rows.length, updates: 20,
  fullTreeMs: measure(20, <Box flexDirection="column">{rows}<Text>active delta</Text></Box>),
  activeTreeMs: measure(20, <Box><Text>active delta</Text></Box>),
  paste: { lines: 10000, rawPromptMs: measurePrompt(paste), foldedPromptMs: measurePrompt(reference), referenceLength: reference.length },
  node: process.version, platform: process.platform }))
