// The main process: the window, the calls it makes (rpc.ts) and the bytes
// it fetches (samples.ts), answered by the engine on one Effect runtime.
import * as NodeServices from "@effect/platform-node/NodeServices";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import * as ManagedRuntime from "effect/ManagedRuntime";
import { app, BrowserWindow, dialog, Menu, nativeTheme, protocol } from "electron";
import path from "node:path";

import { SCHEME } from "../shared/contract.ts";
import * as Engine from "./Engine.ts";
import * as Rpc from "./rpc.ts";
import * as Samples from "./samples.ts";
import * as Updater from "./Updater.ts";

protocol.registerSchemesAsPrivileged([
  {
    scheme: SCHEME,
    privileges: { standard: true, secure: true, supportFetchAPI: true, corsEnabled: true },
  },
]);

const engine = app.isPackaged
  ? path.join(process.resourcesPath, "samplary-engine")
  : path.join(app.getAppPath(), "target", "release", "samplary-engine");
const model = app.isPackaged
  ? path.join(process.resourcesPath, "clap-audio.onnx")
  : path.join(app.getAppPath(), "models", "clap-audio.onnx");

const MainLayer = Rpc.layer.pipe(
  Layer.provideMerge(
    Engine.layer(engine, path.join(app.getPath("userData"), "library.sqlite"), model),
  ),
  Layer.provide(Updater.layer),
  Layer.provide(NodeServices.layer),
);

const runtime = ManagedRuntime.make(MainLayer);

function createWindow() {
  const window = new BrowserWindow({
    width: 1320,
    height: 840,
    minWidth: 960,
    minHeight: 600,
    titleBarStyle: "hiddenInset",
    trafficLightPosition: { x: 18, y: 19 },
    backgroundColor: nativeTheme.shouldUseDarkColors ? "#0a0a0a" : "#ffffff",
    webPreferences: { preload: path.join(__dirname, "preload.js") },
  });
  // One local page. Nothing in it navigates or opens windows.
  window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  window.webContents.on("will-navigate", (event, url) => {
    if (url !== window.webContents.getURL()) event.preventDefault();
  });
  if (MAIN_WINDOW_VITE_DEV_SERVER_URL) {
    void window.loadURL(MAIN_WINDOW_VITE_DEV_SERVER_URL);
  } else {
    void window.loadFile(path.join(__dirname, `../renderer/${MAIN_WINDOW_VITE_NAME}/index.html`));
  }
}

app.on("ready", () => {
  Menu.setApplicationMenu(
    Menu.buildFromTemplate([
      { role: "appMenu" },
      {
        role: "fileMenu",
        submenu: [
          // The folders are watched, so this is for what the watching misses.
          {
            label: "Rescan Folders",
            click: () => void runtime.runPromise(Engine.Engine.use((e) => e.rescan)),
          },
          { type: "separator" },
          { role: "close" },
        ],
      },
      { role: "editMenu" },
      // Reloading and the inspector are for development.
      ...(app.isPackaged ? [] : [{ role: "viewMenu" as const }]),
      { role: "windowMenu" },
    ]),
  );
  protocol.handle(SCHEME, (request) => runtime.runPromise(Samples.handle(request)));
  runtime.runPromise(Effect.void).then(createWindow, (error: unknown) => {
    dialog.showErrorBox("Samplary couldn't start", String(error));
    app.quit();
  });
});

app.on("window-all-closed", () => app.quit());

app.on("activate", () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow();
});

// Closing the runtime stops the engine.
let closing = false;
app.on("will-quit", (event) => {
  event.preventDefault();
  if (closing) return;
  closing = true;
  void runtime.dispose().finally(() => app.exit());
});
