const { contextBridge, ipcRenderer, webUtils } = require('electron')

contextBridge.exposeInMainWorld('desktop', {
  request: (method, params = {}) => ipcRenderer.invoke('desktop:request', method, params),
  chooseWorkspace: () => ipcRenderer.invoke('desktop:choose-workspace'),
  chooseAcceptanceFile: () => ipcRenderer.invoke('desktop:choose-acceptance'),
  // Local path of a file dropped onto the window (File.path is gone in sandboxed renderers).
  getPathForFile: (file) => { try { return webUtils.getPathForFile(file) || '' } catch { return '' } },
  onEvent: (listener) => {
    const handler = (_event, payload) => listener(payload)
    ipcRenderer.on('desktop:event', handler)
    return () => ipcRenderer.removeListener('desktop:event', handler)
  },
})
