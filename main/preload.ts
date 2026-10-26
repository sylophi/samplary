// What the page may ask of Electron: the calls' channel (main/rpc.ts), and
// where a dropped file lives.
import { contextBridge, ipcRenderer, webUtils } from "electron";

const CHANNEL = "rpc";

// Each load of the page makes its calls through a port of its own, which
// closes when the page goes.
const { port1, port2 } = new MessageChannel();
ipcRenderer.postMessage(CHANNEL, null, [port2]);
port1.start();

const bridge = {
  send: (message: unknown) => port1.postMessage(message),
  listen: (f: (message: unknown) => void) => {
    const listener = (event: MessageEvent) => f(event.data);
    port1.addEventListener("message", listener);
    return () => port1.removeEventListener("message", listener);
  },
  pathForFile: (file: File) => webUtils.getPathForFile(file),
};

export type Bridge = typeof bridge;

contextBridge.exposeInMainWorld("bridge", bridge);
