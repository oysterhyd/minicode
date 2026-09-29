import { defineConfig, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Production pages only load local code. Dev keeps Vite's inline HMR preamble working.
const csp: Plugin = {
  name: 'minicode-csp',
  apply: 'build',
  transformIndexHtml: html => html.replace('<head>', `<head>\n    <meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'" />`),
}

export default defineConfig({
  plugins: [react(), tailwindcss(), csp],
  base: './',
  build: { outDir: 'dist', chunkSizeWarningLimit: 1200 },
})
