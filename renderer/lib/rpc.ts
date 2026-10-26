// The page's end of the calls (main/rpc.ts): Effect RPC over the preload's
// message port, on one runtime for the life of the page.
import * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as Fiber from "effect/Fiber";
import * as FiberSet from "effect/FiberSet";
import * as Layer from "effect/Layer";
import * as ManagedRuntime from "effect/ManagedRuntime";
import * as RpcClient from "effect/rpc/RpcClient";
import type { RpcClientError } from "effect/rpc/RpcClientError";
import type * as RpcMessage from "effect/rpc/RpcMessage";
import type * as RpcSerialization from "effect/rpc/RpcSerialization";
import * as Schema from "effect/Schema";
import * as Stream from "effect/Stream";

import type { Bridge } from "../../main/preload.ts";
import { Rpcs } from "../../shared/contract.ts";

declare global {
  interface Window {
    bridge: Bridge;
  }
}

const protocol = RpcClient.Protocol.make(
  Effect.fnUntraced(function* (writeResponse, clientIds) {
    const runFork = yield* FiberSet.makeRuntime<never>();
    const stop = window.bridge.listen((message) => {
      for (const id of clientIds) {
        runFork(writeResponse(id, message as RpcMessage.FromServerEncoded));
      }
    });
    yield* Effect.addFinalizer(() => Effect.sync(stop));
    return {
      send: (_, request) => Effect.sync(() => window.bridge.send(request)),
      supportsAck: true,
      supportsTransferables: false,
      codecFor: Schema.toCodecJson as RpcSerialization.CodecFor,
    };
  }),
);

class Client extends Context.Service<Client, RpcClient.FromGroup<typeof Rpcs, RpcClientError>>()(
  "samplary/Client",
) {}

const client = Effect.gen(function* () {
  return yield* Client;
});

const runtime = ManagedRuntime.make(
  Layer.effect(Client, RpcClient.make(Rpcs)).pipe(
    Layer.provide(Layer.effect(RpcClient.Protocol, protocol)),
  ),
);

/** Makes a call, settling with its answer. */
export const call = <A, E>(f: (client: Client["Service"]) => Effect.Effect<A, E>) =>
  runtime.runPromise(Effect.flatMap(client, f));

/** Follows a streaming call until the returned function is called. */
export const watch = <A, E>(
  f: (client: Client["Service"]) => Stream.Stream<A, E>,
  onValue: (value: A) => void,
) => {
  const fiber = runtime.runFork(
    Stream.unwrap(Effect.map(client, f)).pipe(
      Stream.runForEach((value) => Effect.sync(() => onValue(value))),
    ),
  );
  return () => void runtime.runFork(Fiber.interrupt(fiber));
};
