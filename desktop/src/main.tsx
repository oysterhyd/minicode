import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { ToastProvider } from './components/ui/Toast'
import { TooltipLayer } from './components/ui/Tooltip'
import { ErrorBoundary } from './components/ui/ErrorBoundary'
import './styles/index.css'

ReactDOM.createRoot(document.getElementById('root')!).render(<React.StrictMode>
  <ErrorBoundary label="MiniCode 遇到了问题">
    <ToastProvider><App /><TooltipLayer /></ToastProvider>
  </ErrorBoundary>
</React.StrictMode>)
