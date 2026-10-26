// What the window and the main process say to each other: the calls, as
// Effect RPC over Electron's IPC (main/rpc.ts, renderer/lib/rpc.ts), and
// the bytes (waveforms, audio), fetched from the `sample:` scheme
// (main/samples.ts). The engine (engine/src/server.rs) speaks the same
// shapes as JSON.
import * as Rpc from "effect/rpc/Rpc";
import * as RpcGroup from "effect/rpc/RpcGroup";
import * as Schema from "effect/Schema";

export const Kind = Schema.Literals(["oneShot", "loop"]);
export type Kind = typeof Kind.Type;

export const SortKey = Schema.Literals([
  "name",
  "folder",
  "duration",
  "pitch",
  "tempo",
  "category",
  "added",
  "shuffle",
]);
export type SortKey = typeof SortKey.Type;

/** Spectrum, timbre, pitch, amplitude: how much each counts in a search. */
export const Weights = Schema.Tuple([Schema.Number, Schema.Number, Schema.Number, Schema.Number]);
export type Weights = typeof Weights.Type;

export const Row = Schema.Struct({
  id: Schema.Number,
  name: Schema.String,
  path: Schema.String,
  folder: Schema.String,
  duration: Schema.Number,
  sampleRate: Schema.Number,
  channels: Schema.Number,
  bits: Schema.NullOr(Schema.Number),
  category: Schema.NullOr(Schema.String),
  categoryGuessed: Schema.Boolean,
  /** What the user chose it to be, over its name and sound. */
  chosen: Schema.NullOr(
    Schema.Struct({ category: Schema.String, subcategory: Schema.NullOr(Schema.String) }),
  ),
  /** What kind of its category it is: an open hihat, an 808 kick. */
  subcategory: Schema.NullOr(Schema.String),
  subcategoryGuessed: Schema.Boolean,
  looped: Schema.Boolean,
  note: Schema.NullOr(Schema.String),
  key: Schema.NullOr(Schema.String),
  tempo: Schema.NullOr(Schema.Number),
  favorite: Schema.Boolean,
  similarity: Schema.NullOr(Schema.Number),
});
export type Row = typeof Row.Type;

export const Query = Schema.Struct({
  text: Schema.String,
  folder: Schema.NullOr(Schema.Number),
  dir: Schema.NullOr(Schema.String),
  category: Schema.NullOr(Schema.String),
  /** Only with `category`, one of its subcategories. */
  subcategory: Schema.NullOr(Schema.String),
  kind: Schema.NullOr(Kind),
  favorites: Schema.Boolean,
  similar: Schema.NullOr(Schema.Number),
  weights: Weights,
  sort: SortKey,
  descending: Schema.Boolean,
  /** Which order a shuffle is in. */
  seed: Schema.Number,
});
export type Query = typeof Query.Type;

/** One page of a search's results, and how many there are in all. */
export const SearchResult = Schema.Struct({
  total: Schema.Number,
  rows: Schema.Array(Row),
});
export type SearchResult = typeof SearchResult.Type;

export const FolderInfo = Schema.Struct({
  id: Schema.Number,
  name: Schema.String,
  path: Schema.String,
  count: Schema.Number,
  dirs: Schema.Array(Schema.Struct({ path: Schema.String, count: Schema.Number })),
});
export type FolderInfo = typeof FolderInfo.Type;

export const Progress = Schema.Struct({
  /** Analyzing new files, then listening to them to tell what they are. */
  stage: Schema.Literals(["analyzing", "listening"]),
  done: Schema.Number,
  total: Schema.Number,
});
export type Progress = typeof Progress.Type;

export const Overview = Schema.Struct({
  total: Schema.Number,
  favorites: Schema.Number,
  folders: Schema.Array(FolderInfo),
  categories: Schema.Array(
    Schema.Struct({
      name: Schema.String,
      count: Schema.Number,
      subcategories: Schema.Array(Schema.Struct({ name: Schema.String, count: Schema.Number })),
    }),
  ),
  /** Counts up whenever the samples or what's known about them change. */
  version: Schema.Number,
});
export type Overview = typeof Overview.Type;

export class UnreadableSound extends Schema.TaggedError<UnreadableSound>()("UnreadableSound", {
  reason: Schema.String,
}) {
  override get message(): string {
    return this.reason;
  }
}

const Paths = { paths: Schema.Array(Schema.String) };

export class Rpcs extends RpcGroup.make(
  /** The library's folders, categories and counts. */
  Rpc.make("overview", { success: Overview, stream: true }),
  /** How far the scan has got, or null when there's none. */
  Rpc.make("progress", { success: Schema.NullOr(Progress), stream: true }),
  Rpc.make("addFolders", { payload: Paths }),
  Rpc.make("removeFolder", { payload: { id: Schema.Number } }),
  /**
   * Folders join the library. The first sound comes back as a row to find
   * ones like it, from outside the library if it isn't in it.
   */
  Rpc.make("dropPaths", { payload: Paths, success: Schema.NullOr(Row), error: UnreadableSound }),
  /** The results from `offset`, at most `limit` of them. */
  Rpc.make("search", {
    payload: { query: Query, offset: Schema.Number, limit: Schema.Number },
    success: SearchResult,
  }),
  Rpc.make("setFavorite", { payload: { id: Schema.Number, favorite: Schema.Boolean } }),
  /**
   * A menu, at `x`, `y` in the window, to choose what the sample is over
   * what its name or sound say. The row as it is now, or null if nothing
   * was chosen.
   */
  Rpc.make("chooseCategory", {
    payload: { row: Row, x: Schema.Number, y: Schema.Number },
    success: Schema.NullOr(Row),
  }),
  // The window's own: dialogs, Finder, and dragging files out.
  Rpc.make("chooseFolders", { success: Schema.Array(Schema.String) }),
  Rpc.make("confirm", {
    payload: { message: Schema.String, detail: Schema.String, action: Schema.String },
    success: Schema.Boolean,
  }),
  Rpc.make("alert", { payload: { message: Schema.String, detail: Schema.String } }),
  Rpc.make("reveal", { payload: { path: Schema.String } }),
  /** `icon` is a PNG data URL at twice its size, drawn under the pointer. */
  Rpc.make("startDrag", { payload: { path: Schema.String, icon: Schema.String } }),
  /** Whether a newer version has downloaded. */
  Rpc.make("updateReady", { success: Schema.Boolean, stream: true }),
  /** Restarts into the newer version. */
  Rpc.make("installUpdate"),
) {}

export const SCHEME = "sample";

/** Thumbnail waveforms for `ids`, back to back. */
export const thumbnailsUrl = (ids: readonly number[]) => `${SCHEME}://thumbnails/${ids.join(",")}`;

/** A detailed waveform, `columns` wide. */
export const waveformUrl = (id: number, columns: number) =>
  `${SCHEME}://waveform/${id}?columns=${columns}`;

/**
 * The sample's audio, one channel after another as 32-bit floats, with its
 * rate and channel count in the `x-sample-rate` and `x-channels` headers.
 */
export const pcmUrl = (id: number) => `${SCHEME}://pcm/${id}`;

export const THUMB_COLUMNS = 256;
/** min, max, then low / mid / high band balance. */
export const BYTES_PER_COLUMN = 5;
/** Longer files are drawn and played from their first ten minutes. */
export const MAX_DECODE_SECONDS = 600;
