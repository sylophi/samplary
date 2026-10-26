// Serves the contract's calls (shared/contract.ts) to the windows, as
// Effect RPC over a message port from each load of a page: each message a
// structured clone of what the JSON codec makes of it, each port a client.
import * as Effect from "effect/Effect";
import * as FiberSet from "effect/FiberSet";
import * as Layer from "effect/Layer";
import * as Queue from "effect/Queue";
import type * as RpcMessage from "effect/rpc/RpcMessage";
import type * as RpcSerialization from "effect/rpc/RpcSerialization";
import * as RpcServer from "effect/rpc/RpcServer";
import * as Schema from "effect/Schema";
import {
  BrowserWindow,
  dialog,
  type IpcMainEvent,
  ipcMain,
  Menu,
  type MenuItemConstructorOptions,
  type MessagePortMain,
  nativeImage,
  shell,
  type WebContents,
} from "electron";

import { type Row, Rpcs } from "../shared/contract.ts";
import * as Engine from "./Engine.ts";
import * as Updater from "./Updater.ts";

export const CHANNEL = "rpc";

/** Each client's page and the port its calls come through. */
const clients = new Map<number, { contents: WebContents; port: MessagePortMain }>();

const protocol = RpcServer.Protocol.make(
  Effect.fnUntraced(function* (writeRequest) {
    const runFork = yield* FiberSet.makeRuntime<never>();
    const disconnects = yield* Queue.unbounded<number>();
    let next = 0;
    const onPort = (event: IpcMainEvent) => {
      const [port] = event.ports;
      const id = next++;
      clients.set(id, { contents: event.sender, port });
      port.on("message", ({ data }) =>
        runFork(writeRequest(id, data as RpcMessage.FromClientEncoded)),
      );
      // The page went, or was reloaded: what it asked for stops.
      port.on("close", () => {
        clients.delete(id);
        Queue.offerUnsafe(disconnects, id);
      });
      port.start();
    };
    ipcMain.on(CHANNEL, onPort);
    yield* Effect.addFinalizer(() => Effect.sync(() => ipcMain.off(CHANNEL, onPort)));
    return {
      disconnects,
      send: (id, response) => Effect.sync(() => clients.get(id)?.port.postMessage(response)),
      end: () => Effect.void,
      clientIds: Effect.sync(() => new Set(clients.keys())),
      initialMessage: Effect.succeedNone,
      supportsAck: true,
      supportsTransferables: false,
      supportsSpanPropagation: false,
      supportsNotifications: false,
      codecFor: Schema.toCodecJson as RpcSerialization.CodecFor,
    };
  }),
);

/** The window a call came from: calls only come from pages in one. */
const windowOf = (id: number) => BrowserWindow.fromWebContents(clients.get(id)!.contents)!;

type Choice = Row["chosen"];

/**
 * Shows the categories, and the kinds of each, at `x`, `y`, checking what
 * `row` was chosen to be. Undefined if nothing was picked.
 */
const pickCategory = (
  window: BrowserWindow,
  categories: ReadonlyArray<Engine.Category>,
  row: Row,
  x: number,
  y: number,
) =>
  new Promise<Choice | undefined>((resolve) => {
    const { chosen } = row;
    const item = (label: string, choice: Choice, checked: boolean): MenuItemConstructorOptions => ({
      label,
      type: "checkbox",
      checked,
      click: () => resolve(choice),
    });
    const menu = Menu.buildFromTemplate([
      item("automatic", null, chosen === null),
      { type: "separator" },
      ...categories.map(({ name, subcategories }): MenuItemConstructorOptions => {
        if (subcategories.length === 0) {
          return item(name, { category: name, subcategory: null }, chosen?.category === name);
        }
        return {
          label: name,
          submenu: [
            item(
              "any kind",
              { category: name, subcategory: null },
              chosen?.category === name && chosen.subcategory === null,
            ),
            { type: "separator" },
            ...subcategories.map((t) =>
              item(
                t,
                { category: name, subcategory: t },
                chosen?.category === name && chosen.subcategory === t,
              ),
            ),
          ],
        };
      }),
    ]);
    // A click comes before the menu says it's closed.
    menu.popup({ window, x: Math.round(x), y: Math.round(y), callback: () => resolve(undefined) });
  });

const handlers = Rpcs.toLayer(
  Effect.gen(function* () {
    const engine = yield* Engine.Engine;
    const updater = yield* Updater.Updater;
    return Rpcs.of({
      overview: () => engine.overview,
      progress: () => engine.progress,
      addFolders: ({ paths }) => engine.addFolders(paths),
      removeFolder: ({ id }) => engine.removeFolder(id),
      dropPaths: ({ paths }) => engine.dropPaths(paths),
      search: ({ query, offset, limit }) => engine.search(query, offset, limit),
      setFavorite: ({ id, favorite }) => engine.setFavorite(id, favorite),
      chooseCategory: ({ row, x, y }, { client }) =>
        Effect.gen(function* () {
          const categories = yield* engine.categories;
          const choice = yield* Effect.promise(() =>
            pickCategory(windowOf(client.id), categories, row, x, y),
          );
          if (choice === undefined) return null;
          return yield* engine.setCategory(
            row.id,
            choice?.category ?? null,
            choice?.subcategory ?? null,
          );
        }),
      chooseFolders: (_, { client }) =>
        Effect.promise(async () => {
          const picked = await dialog.showOpenDialog(windowOf(client.id), {
            title: "Add Sample Folders",
            buttonLabel: "Add",
            properties: ["openDirectory", "multiSelections"],
          });
          return picked.canceled ? [] : picked.filePaths;
        }),
      confirm: ({ message, detail, action }, { client }) =>
        Effect.promise(async () => {
          const { response } = await dialog.showMessageBox(windowOf(client.id), {
            type: "warning",
            message,
            detail,
            buttons: [action, "Cancel"],
            defaultId: 0,
            cancelId: 1,
          });
          return response === 0;
        }),
      alert: ({ message, detail }, { client }) =>
        Effect.promise(() =>
          dialog.showMessageBox(windowOf(client.id), { type: "error", message, detail }),
        ).pipe(Effect.asVoid),
      reveal: ({ path }) => Effect.sync(() => shell.showItemInFolder(path)),
      startDrag: ({ path, icon }, { client }) =>
        Effect.sync(() =>
          clients.get(client.id)?.contents.startDrag({
            file: path,
            // Drawn at twice its size, for Retina screens.
            icon: nativeImage.createFromBuffer(Buffer.from(icon.split(",")[1], "base64"), {
              scaleFactor: 2,
            }),
          }),
        ),
      updateReady: () => updater.ready,
      installUpdate: () => updater.install,
    });
  }),
);

export const layer = RpcServer.layer(Rpcs, { disableFatalDefects: true }).pipe(
  Layer.provide(handlers),
  Layer.provide(Layer.effect(RpcServer.Protocol, protocol)),
);
