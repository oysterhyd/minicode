const test = require('node:test')
const assert = require('node:assert/strict')
const utils = require('../electron/git-utils.cjs')

test('parseNumstat reads plain, binary and renamed entries', () => {
  const stdout = ['3\t1\tsrc/a.ts', '-\t-\timg/logo.png', '5\t0\t', 'old/name.ts', 'new/name.ts', '12\t4\tsrc/中文.md', ''].join('\0')
  const stats = utils.parseNumstat(stdout)
  assert.deepEqual(stats.get('src/a.ts'), { additions: 3, deletions: 1 })
  assert.deepEqual(stats.get('img/logo.png'), { additions: 0, deletions: 0 })
  assert.deepEqual(stats.get('new/name.ts'), { additions: 5, deletions: 0 })
  assert.equal(stats.has('old/name.ts'), false)
  assert.deepEqual(stats.get('src/中文.md'), { additions: 12, deletions: 4 })
})

test('parseNumstat tolerates empty and malformed output', () => {
  assert.equal(utils.parseNumstat('').size, 0)
  assert.equal(utils.parseNumstat(undefined).size, 0)
  assert.equal(utils.parseNumstat('garbage\0').size, 0)
})

test('parseStatus skips the original path of renames', () => {
  const stdout = [' M src/a.ts', 'R  new.ts', 'old.ts', '?? notes.txt', ''].join('\0')
  assert.deepEqual(utils.parseStatus(stdout), [
    { status: ' M', path: 'src/a.ts' }, { status: 'R ', path: 'new.ts' }, { status: '??', path: 'notes.txt' },
  ])
})

test('countLines counts text lines and treats binary as zero', () => {
  assert.equal(utils.countLines(Buffer.from('')), 0)
  assert.equal(utils.countLines(Buffer.from('a\nb\n')), 2)
  assert.equal(utils.countLines(Buffer.from('a\nb')), 2)
  assert.equal(utils.countLines(Buffer.from([0x61, 0x00, 0x0a])), 0)
})

test('isAllowedExternal filters protocols', () => {
  for (const url of ['https://a.dev', 'http://localhost:3000/x', 'mailto:a@b.c']) assert.equal(utils.isAllowedExternal(url), true, url)
  for (const url of ['file:///etc/passwd', 'javascript:alert(1)', 'data:text/html,x', 'ms-settings:', 'not a url', '', null]) {
    assert.equal(utils.isAllowedExternal(url), false, String(url))
  }
})

test('isAppUrl allows only the dev server or the bundled index', () => {
  assert.equal(utils.isAppUrl('http://127.0.0.1:5173/', 'http://127.0.0.1:5173'), true)
  assert.equal(utils.isAppUrl('http://127.0.0.1:5173.evil.com/', 'http://127.0.0.1:5173'), false)
  assert.equal(utils.isAppUrl('https://example.com', 'http://127.0.0.1:5173'), false)
  assert.equal(utils.isAppUrl('file:///D:/app/desktop/dist/index.html#x'), true)
  assert.equal(utils.isAppUrl('file:///D:/other.html'), false)
  assert.equal(utils.isAppUrl('https://example.com/dist/index.html'), false)
})

test('recent workspace helpers migrate, dedupe and cap the list', () => {
  assert.deepEqual(utils.normalizeSettings({ workspace: '/a' }), { workspace: '/a', recent: ['/a'] })
  assert.deepEqual(utils.normalizeSettings(null), { workspace: null, recent: [] })
  assert.deepEqual(utils.normalizeSettings({ workspace: '/b', recent: ['/a', '/b', 3] }), { workspace: '/b', recent: ['/b', '/a'] })
  const many = Array.from({ length: 20 }, (_, i) => `/w${i}`)
  assert.equal(utils.addRecent(many, '/new').length, utils.RECENT_LIMIT)
  assert.equal(utils.addRecent(many, '/new')[0], '/new')
  assert.deepEqual(utils.removeRecent(['/a', '/b'], '/a'), ['/b'])
})

test('boundsVisible rejects windows that are off every display', () => {
  const areas = [{ x: 0, y: 0, width: 1920, height: 1080 }]
  assert.equal(utils.boundsVisible({ x: 100, y: 100, width: 1200, height: 800 }, areas), true)
  assert.equal(utils.boundsVisible({ x: 5000, y: 100, width: 1200, height: 800 }, areas), false)
  assert.equal(utils.boundsVisible({ x: 1900, y: 100, width: 1200, height: 800 }, areas), false)
  assert.equal(utils.boundsVisible(null, areas), false)
  assert.equal(utils.boundsVisible({ x: 0, y: 0, width: 800, height: 600 }, []), false)
})
