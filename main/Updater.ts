// Updates, through Electron's autoUpdater (Squirrel.Mac) from
// update.electronjs.org, which answers from the repo's GitHub releases. A
// newer release downloads in the background and is installed when the app
// next quits, or at once on a restart.
import * as Context from "effect/Context";
import * as Effect from "effect/Effect";
import * as Layer from "effect/Layer";
import * as Schedule from "effect/Schedule";
import type * as Stream from "effect/Stream";
import * as SubscriptionRef from "effect/SubscriptionRef";
import { app, autoUpdater } from "electron";

/** Whether this is a Developer ID build (vite.main.config.mts). */
declare const UPDATES: boolean;

export class Updater extends Context.Service<
  Updater,
  {
    /** Whether a newer version has downloaded. */
    readonly ready: Stream.Stream<boolean>;
    /** Quits, installs it and opens the new version. */
    readonly install: Effect.Effect<void>;
  }
>()("samplary/Updater") {}

const FEED = `https://update.electronjs.org/sylophi/samplary/${process.platform}-${process.arch}/${app.getVersion()}`;

const downloaded = Effect.callback<void>((resume) => {
  const listener = () => resume(Effect.void);
  autoUpdater.once("update-downloaded", listener);
  return Effect.sync(() => autoUpdater.off("update-downloaded", listener));
});

export const layer = Layer.effect(
  Updater,
  Effect.gen(function* () {
    const ready = yield* SubscriptionRef.make(false);
    if (UPDATES && app.isPackaged) {
      yield* Effect.sync(() => {
        autoUpdater.on("error", (error) => console.error("couldn't update:", error.message));
        autoUpdater.setFeedURL({ url: FEED });
      }).pipe(
        // Checks hourly until an update has downloaded.
        Effect.andThen(
          Effect.race(
            Effect.sync(() => autoUpdater.checkForUpdates()).pipe(
              Effect.repeat(Schedule.spaced("1 hour")),
            ),
            downloaded,
          ),
        ),
        Effect.andThen(SubscriptionRef.set(ready, true)),
        Effect.catchCause((cause) => Effect.logError("updates stopped", cause)),
        Effect.forkScoped,
      );
    }
    return Updater.of({
      ready: SubscriptionRef.changes(ready),
      install: Effect.sync(() => autoUpdater.quitAndInstall()),
    });
  }),
);
