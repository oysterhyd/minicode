const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('desktop', {
  request: (method, params = {}) => ipcRenderer.invoke('desktop:request', method, params),
  chooseWorkspace: () => ipcRenderer.invoke('desktop:choose-workspace'),
  chooseAcceptanceFile: () => ipcRenderer.invoke('desktop:choose-acceptance'),
  onEvent: (listener) => {
    const handler = (_event, payload) => listener(payload)
    ipcRenderer.on('desktop:event', handler)
    return () => ipcRenderer.removeListener('desktop:event', handler)
  },
})
