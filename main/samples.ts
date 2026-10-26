// The `sample:` scheme: waveforms and audio for the page to fetch, as bytes
// rather than through the calls' JSON (shared/contract.ts has the URLs).
import * as Effect from "effect/Effect";

import { THUMB_COLUMNS } from "../shared/contract.ts";
import * as Engine from "./Engine.ts";

const CORS = {
  "access-control-allow-origin": "*",
  "access-control-expose-headers": "x-sample-rate, x-channels",
};

const bytes = (body: Uint8Array, headers: Record<string, string> = {}) =>
  new Response(body as Uint8Array<ArrayBuffer>, {
    headers: { ...CORS, "content-type": "application/octet-stream", ...headers },
  });

const failure = (status: number, reason: string) => new Response(reason, { status, headers: CORS });

export const handle = Effect.fn("samples.handle")(
  function* (request: Request) {
    const engine = yield* Engine.Engine;
    const url = new URL(request.url);
    const param = url.pathname.slice(1);
    if (url.host === "thumbnails") {
      return bytes(yield* engine.thumbnails(param.split(",").filter(Boolean).map(Number)));
    }
    if (url.host === "waveform") {
      const columns = Number(url.searchParams.get("columns") ?? THUMB_COLUMNS);
      return bytes(yield* engine.waveform(Number(param), columns));
    }
    if (url.host === "pcm") {
      const pcm = yield* engine.pcm(Number(param));
      return bytes(pcm.data, {
        "x-sample-rate": String(pcm.sampleRate),
        "x-channels": String(pcm.channels),
      });
    }
    return failure(404, "Not found.");
  },
  Effect.catchTag("UnreadableSound", (e) => Effect.succeed(failure(422, e.reason))),
);
