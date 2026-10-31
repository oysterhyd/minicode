import assert from 'node:assert/strict'
import test from 'node:test'
import React from 'react'
import { Text } from 'ink'
import { render } from 'ink-testing-library'
import { TranscriptLog } from '../src/components/TranscriptLog.js'
import { delay } from './fake-bridge.js'

test('T-UI9 1200 completed rows render once and stay outside live reconciliation', async () => {
  let renders = 0
  const Row = ({ text }: { text: string }) => { renders++; return <Text>{text}</Text> }
  const items = Array.from({ length: 1200 }, (_, i) => ({ id: String(i), text: `row ${i}` }))
  const view = render(<TranscriptLog items={items} epoch={0} columns={80} renderItem={item => <Row text={item.text} />} />)
  try {
    await delay()
    assert.equal(renders, 1200)
    for (let i = 0; i < 10; i++) view.rerender(<TranscriptLog items={items} epoch={0} columns={80} renderItem={item => <Row text={item.text} />} />)
    await delay()
    assert.equal(renders, 1200)
    assert.ok(view.frames.join('\n').includes('row 1199'))
  } finally { view.unmount() }
})
test('T-UI9 prepended history and corrected sealed rows are printed once', async () => {
  let renders=0
  const Row=({text}:{text:string})=>{renders++;return <Text>{text}</Text>}
  const a={id:'a',text:'original'},b={id:'b',text:'settled'}
  const view=render(<TranscriptLog items={[a,b]} epoch={0} columns={80} renderItem={item=><Row text={item.text}/>} />)
  try {
    await delay()
    view.rerender(<TranscriptLog items={[{id:'earlier',text:'earlier history'},{...a,text:'corrected'},b]} epoch={0} columns={80} renderItem={item=><Row text={item.text}/>} />)
    await delay()
    assert.equal(renders,4)
    assert.match(view.frames.join('\n'),/earlier history/)
    assert.match(view.frames.join('\n'),/corrected/)
  } finally {view.unmount()}
})
