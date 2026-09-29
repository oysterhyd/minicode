// Apply the saved theme before the first paint so the window never flashes the wrong colors.
try {
  const prefs = JSON.parse(localStorage.getItem('minicode.preferences.v2') || 'null')
  const legacy = localStorage.getItem('minicode.theme')
  const choice = prefs && prefs.theme ? prefs.theme : legacy === 'light' || legacy === 'dark' ? legacy : 'system'
  const dark = choice === 'dark' || (choice === 'system' && matchMedia('(prefers-color-scheme: dark)').matches)
  document.documentElement.dataset.theme = dark ? 'dark' : 'light'
  if (prefs && prefs.fontScale) document.documentElement.style.setProperty('--ui-scale', String(prefs.fontScale))
} catch { document.documentElement.dataset.theme = 'light' }
