// The engine (engine/src/server.rs): the library, scanning, search and
// audio, in a process of its own so nothing here waits on them. It takes
// one JSON request per line on stdin and answers each by id on stdout. An
// answer in bytes is a line saying how many, then the bytes.
import * as Context from "effect/Context";
import * as Deferred from "effect/Deferred";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import * as ChildProcess from "effect/process/ChildProcess";
import * as ChildProcessSpawner from "effect/process/ChildProcessSpawner";
import * as Queue from "effect/Queue";
import * as Schema from "effect/Schema";
import * as Stream from "effect/Stream";
import * as SubscriptionRef from "effect/SubscriptionRef";

import {
  Overview,
  Progress,
  type Query,
  Row,
  SearchResult,
  UnreadableSound,
} from "../shared/contract.ts";

/** A sound's channels, one after another as 32-bit floats. */
export interface Pcm {
  readonly sampleRate: number;
  readonly channels: number;
  readonly data: Uint8Array;
}

export class Engine extends Context.Service<
  Engine,
  {
    /** The library's folders, categories and counts. */
    readonly overview: Stream.Stream<Overview>;
    /** How far the scan has got, or null when there's none. */
    readonly progress: Stream.Stream<Progress | null>;
    readonly addFolders: (paths: ReadonlyArray<string>) => Effect.Effect<void>;
    readonly removeFolder: (id: number) => Effect.Effect<void>;
    readonly rescan: Effect.Effect<void>;
    /**
     * Folders join the library. The first sound comes back as a row to find
     * ones like it.
     */
    readonly dropPaths: (
      paths: ReadonlyArray<string>,
    ) => Effect.Effect<Row | null, UnreadableSound>;
    readonly search: (query: Query, offset: number, limit: number) => Effect.Effect<SearchResult>;
    readonly setFavorite: (id: number, favorite: boolean) => Effect.Effect<void>;
    /** Every category, and the kinds of each. */
    readonly categories: Effect.Effect<ReadonlyArray<Category>>;
    /**
     * Makes the sample `category`, and `subcategory` of it, over what its
     * name says. With no category, it's the name's again. The row as it is
     * now.
     */
    readonly setCategory: (
      id: number,
      category: string | null,
      subcategory: string | null,
    ) => Effect.Effect<Row | null>;
    /** Thumbnail waveforms for `ids`, back to back. */
    readonly thumbnails: (ids: ReadonlyArray<number>) => Effect.Effect<Uint8Array>;
    readonly waveform: (id: number, columns: number) => Effect.Effect<Uint8Array, UnreadableSound>;
    readonly pcm: (id: number) => Effect.Effect<Pcm, UnreadableSound>;
  }
>()("samplary/Engine") {}

const Category = Schema.Struct({
  name: Schema.String,
  subcategories: Schema.Array(Schema.String),
});
export type Category = typeof Category.Type;

type Message =
  | { readonly id: number; readonly ok: unknown }
  | { readonly id: number; readonly bytes: number; readonly meta: unknown }
  | { readonly id: number; readonly error: string }
  | { readonly event: "overview" | "progress"; readonly data: unknown };

type Reply = { readonly ok: unknown } | { readonly data: Uint8Array; readonly meta: unknown };

const concat = (chunks: ReadonlyArray<Uint8Array>) => {
  if (chunks.length === 1) return chunks[0];
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    out.set(c, at);
    at += c.length;
  }
  return out;
};

/** What the engine writes, as messages, each with its bytes if it has some. */
const messages = (stdout: Stream.Stream<Uint8Array, unknown>) =>
  Stream.suspend(() => {
    const decoder = new TextDecoder();
    // The start of a line not yet ended.
    let line = new Uint8Array(0);
    // A message whose bytes are still arriving.
    let body: { message: Message; chunks: Uint8Array[]; left: number } | null = null;
    return stdout.pipe(
      Stream.flatMap((chunk) => {
        const out: Array<[Message, Uint8Array | null]> = [];
        while (chunk.length > 0) {
          if (body) {
            const take = Math.min(body.left, chunk.length);
            body.chunks.push(chunk.subarray(0, take));
            body.left -= take;
            chunk = chunk.subarray(take);
            if (body.left === 0) {
              out.push([body.message, concat(body.chunks)]);
              body = null;
            }
            continue;
          }
          const end = chunk.indexOf(10);
          if (end < 0) {
            line = concat([line, chunk]).slice();
            break;
          }
          const message = JSON.parse(
            decoder.decode(concat([line, chunk.subarray(0, end)])),
          ) as Message;
          line = new Uint8Array(0);
          chunk = chunk.subarray(end + 1);
          if ("bytes" in message && message.bytes > 0) {
            body = { message, chunks: [], left: message.bytes };
          } else {
            out.push([message, "bytes" in message ? new Uint8Array(0) : null]);
          }
        }
        return Stream.fromIterable(out);
      }),
    );
  });

const PcmMeta = Schema.Struct({ sampleRate: Schema.Number, channels: Schema.Number });

const make = (binary: string, database: string, model: string) =>
  Effect.gen(function* () {
    const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
    const input = yield* Queue.unbounded<Uint8Array>();
    const handle = yield* spawner
      .spawn(
        ChildProcess.make(binary, ["serve", database, model], {
          stdin: Stream.fromQueue(input),
          stderr: "inherit",
        }),
      )
      .pipe(Effect.orDie);

    const overview = yield* SubscriptionRef.make<Overview | null>(null);
    const progress = yield* SubscriptionRef.make<Progress | null>(null);
    const pending = new Map<number, Deferred.Deferred<Reply, string>>();
    let next = 0;
    let stopped = false;

    const receive = ([message, data]: [Message, Uint8Array | null]) =>
      Effect.gen(function* () {
        if ("event" in message) {
          return yield* message.event === "overview"
            ? Schema.decodeUnknownEffect(Overview)(message.data).pipe(
                Effect.flatMap((o) => SubscriptionRef.set(overview, o)),
              )
            : Schema.decodeUnknownEffect(Schema.NullOr(Progress))(message.data).pipe(
                Effect.flatMap((p) => SubscriptionRef.set(progress, p)),
              );
        }
        const deferred = pending.get(message.id);
        pending.delete(message.id);
        if (!deferred) return;
        if ("error" in message) return yield* Deferred.fail(deferred, message.error);
        yield* Deferred.succeed(
          deferred,
          data
            ? { data, meta: "meta" in message ? message.meta : null }
            : { ok: "ok" in message ? message.ok : null },
        );
      });

    yield* messages(handle.stdout).pipe(
      Stream.runForEach(receive),
      Effect.orDie,
      Effect.ensuring(
        Effect.gen(function* () {
          stopped = true;
          for (const deferred of pending.values()) {
            yield* Deferred.die(deferred, new Error("The engine stopped."));
          }
          pending.clear();
        }),
      ),
      Effect.catchCause((cause) => Effect.logError("the engine stopped", cause)),
      Effect.forkScoped,
    );

    const send = (method: string, params: unknown) =>
      Effect.suspend(() => {
        if (stopped) return Effect.die(new Error("The engine stopped."));
        const id = ++next;
        const deferred = Deferred.makeUnsafe<Reply, string>();
        pending.set(id, deferred);
        const request = JSON.stringify({ id, method, params }) + "\n";
        return Queue.offer(input, new TextEncoder().encode(request)).pipe(
          Effect.andThen(Deferred.await(deferred)),
          Effect.onInterrupt(() => Effect.sync(() => pending.delete(id))),
        );
      });

    /** A call whose answer is JSON, decoded as `schema`. */
    const call = <S extends Schema.Top>(schema: S, method: string, params: unknown = null) =>
      send(method, params).pipe(
        Effect.flatMap((reply) =>
          Schema.decodeUnknownEffect(schema)("ok" in reply ? reply.ok : null).pipe(Effect.orDie),
        ),
      );

    /** A call whose answer is bytes. */
    const bytes = (method: string, params: unknown) =>
      send(method, params).pipe(
        Effect.map((reply) =>
          "data" in reply ? reply : { data: new Uint8Array(0), meta: null as unknown },
        ),
      );

    // Calls the engine only refuses if something is badly wrong.
    const sure = <A, R>(effect: Effect.Effect<A, string, R>) =>
      Effect.catch(effect, (reason) => Effect.die(new Error(reason)));
    const unreadable = <A, R>(effect: Effect.Effect<A, string, R>) =>
      Effect.mapError(effect, (reason) => new UnreadableSound({ reason }));

    return Engine.of({
      overview: SubscriptionRef.changes(overview).pipe(
        Stream.filter((o): o is Overview => o !== null),
      ),
      progress: SubscriptionRef.changes(progress),
      addFolders: (paths) => sure(call(Schema.Null, "addFolders", { paths })),
      removeFolder: (id) => sure(call(Schema.Null, "removeFolder", { id })),
      rescan: sure(call(Schema.Null, "rescan")),
      dropPaths: (paths) => unreadable(call(Schema.NullOr(Row), "dropPaths", { paths })),
      search: (query, offset, limit) =>
        sure(call(SearchResult, "search", { query, offset, limit })),
      setFavorite: (id, favorite) => sure(call(Schema.Null, "setFavorite", { id, favorite })),
      categories: sure(call(Schema.Array(Category), "categories")),
      setCategory: (id, category, subcategory) =>
        sure(call(Schema.NullOr(Row), "setCategory", { id, category, subcategory })),
      thumbnails: (ids) => sure(Effect.map(bytes("thumbnails", { ids }), (reply) => reply.data)),
      waveform: (id, columns) =>
        unreadable(Effect.map(bytes("waveform", { id, columns }), (reply) => reply.data)),
      pcm: (id) =>
        unreadable(bytes("pcm", { id })).pipe(
          Effect.flatMap(({ data, meta }) =>
            Schema.decodeUnknownEffect(PcmMeta)(meta).pipe(
              Effect.map((m) => ({ ...m, data })),
              Effect.orDie,
            ),
          ),
        ),
    });
  });

/**
 * The engine at `binary`, keeping the library in `database` and listening
 * with the CLAP model at `model`.
 */
export const layer = (binary: string, database: string, model: string) =>
  Layer.effect(Engine, make(binary, database, model));
