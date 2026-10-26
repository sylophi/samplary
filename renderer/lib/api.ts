import {
  MAX_DECODE_SECONDS,
  type Overview,
  pcmUrl,
  type Progress,
  type Query,
  type Row,
  thumbnailsUrl,
  waveformUrl,
} from "../../shared/contract.ts";
import { call, watch } from "@/lib/rpc";

export {
  BYTES_PER_COLUMN,
  type FolderInfo as Folder,
  type Kind,
  type Overview,
  type Progress,
  type Query,
  type Row,
  type SortKey,
  THUMB_COLUMNS,
  type Weights,
} from "../../shared/contract.ts";

/** A sound dropped in from outside the library, to find ones like it. */
export const isGuest = (row: Row) => row.id < 0;

/** Waveforms cover at most a file's first ten minutes. */
export const drawnSeconds = (duration: number) => Math.min(duration, MAX_DECODE_SECONDS);

async function bytes(url: string) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(await response.text());
  return response;
}

export const api = {
  watchOverview: (onOverview: (overview: Overview) => void) =>
    watch((c) => c.overview(), onOverview),
  watchProgress: (onProgress: (progress: Progress | null) => void) =>
    watch((c) => c.progress(), onProgress),
  watchUpdateReady: (onReady: (ready: boolean) => void) => watch((c) => c.updateReady(), onReady),
  installUpdate: () => call((c) => c.installUpdate()),
  addFolders: (paths: readonly string[]) => call((c) => c.addFolders({ paths })),
  removeFolder: (id: number) => call((c) => c.removeFolder({ id })),
  dropPaths: (paths: readonly string[]) => call((c) => c.dropPaths({ paths })),
  search: (query: Query, offset: number, limit: number) =>
    call((c) => c.search({ query, offset, limit })),
  setFavorite: (id: number, favorite: boolean) => call((c) => c.setFavorite({ id, favorite })),
  chooseCategory: (row: Row, x: number, y: number) => call((c) => c.chooseCategory({ row, x, y })),
  thumbnails: async (ids: readonly number[]) =>
    new Uint8Array(await (await bytes(thumbnailsUrl(ids))).arrayBuffer()),
  waveform: async (id: number, columns: number) =>
    new Uint8Array(await (await bytes(waveformUrl(id, columns))).arrayBuffer()),
  /** The sample's audio, ready to play. */
  pcm: async (id: number) => {
    const response = await bytes(pcmUrl(id));
    const sampleRate = Number(response.headers.get("x-sample-rate"));
    const channels = Number(response.headers.get("x-channels"));
    const data = new Float32Array(await response.arrayBuffer());
    const length = data.length / channels;
    const buffer = new AudioBuffer({ length, numberOfChannels: channels, sampleRate });
    for (let c = 0; c < channels; c++) {
      buffer.copyToChannel(data.subarray(c * length, (c + 1) * length), c);
    }
    return buffer;
  },
  chooseFolders: () => call((c) => c.chooseFolders()),
  confirm: (message: string, detail: string, action: string) =>
    call((c) => c.confirm({ message, detail, action })),
  alert: (message: string, detail: string) => call((c) => c.alert({ message, detail })),
  reveal: (path: string) => call((c) => c.reveal({ path })),
  startDrag: (path: string, icon: string) => call((c) => c.startDrag({ path, icon })),
  pathForFile: (file: File) => window.bridge.pathForFile(file),
};
