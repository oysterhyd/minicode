import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { ToastProvider } from './components/ui/Toast'
import { TooltipLayer } from './components/ui/Tooltip'
import { ErrorBoundary } from './components/ui/ErrorBoundary'
import './styles/index.css'

// Never let a dropped local HTML file navigate the privileged renderer.
// Bubble-phase prevention leaves the composer's file-mention handlers intact.
for (const name of ['dragover', 'drop']) window.addEventListener(name, event => {
  if ((event as DragEvent).dataTransfer?.types.includes('Files')) event.preventDefault()
})

ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode>
  <ErrorBoundary label="MiniCode 遇到了问题">
    <ToastProvider><App /><TooltipLayer /></ToastProvider>
  </ErrorBoundary>
</React.StrictMode>)
