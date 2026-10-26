import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdir, rename, writeFile } from "node:fs/promises";
import { MakerZIP } from "@electron-forge/maker-zip";
import { FusesPlugin } from "@electron-forge/plugin-fuses";
import { VitePlugin } from "@electron-forge/plugin-vite";
import type { ForgeConfig } from "@electron-forge/shared-types";
import { FuseV1Options, FuseVersion } from "@electron/fuses";

// CLAP's audio half (LAION's clap-htsat-unfused, Apache-2.0), as exported
// to ONNX for transformers.js, pinned to a revision and checked.
const MODEL = "models/clap-audio.onnx";
const MODEL_URL =
  "https://huggingface.co/Xenova/clap-htsat-unfused/resolve/c28f2883575e590e04d3146ff0713c2448d691ba/onnx/audio_model.onnx";
const MODEL_SHA256 = "a1c2b43c44f71e0fa841a4b86700886c199bf87699ea45632c4d831bc6c88957";

async function fetchModel() {
  if (existsSync(MODEL)) return;
  console.log(`Downloading ${MODEL}`);
  const response = await fetch(MODEL_URL);
  if (!response.ok) throw new Error(`${MODEL_URL}: ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const sha = createHash("sha256").update(bytes).digest("hex");
  if (sha !== MODEL_SHA256) throw new Error(`${MODEL_URL} isn't the model expected (${sha})`);
  await mkdir("models", { recursive: true });
  await writeFile(`${MODEL}.part`, bytes);
  await rename(`${MODEL}.part`, MODEL);
}

// A Developer ID, when there is one (as in the release workflow), signs the
// app, and notarizes it given an Apple ID too.
const identity = process.env.APPLE_SIGNING_IDENTITY;
const { APPLE_ID, APPLE_APP_SPECIFIC_PASSWORD, APPLE_TEAM_ID } = process.env;
const notarize =
  APPLE_ID && APPLE_APP_SPECIFIC_PASSWORD && APPLE_TEAM_ID
    ? { appleId: APPLE_ID, appleIdPassword: APPLE_APP_SPECIFIC_PASSWORD, teamId: APPLE_TEAM_ID }
    : undefined;

const config: ForgeConfig = {
  packagerConfig: {
    asar: true,
    icon: "assets/icon",
    appBundleId: "io.github.sylophi.samplary",
    appCategoryType: "public.app-category.music",
    // The engine and its model, beside app.asar.
    extraResource: ["target/release/samplary-engine", MODEL],
    ...(identity ? { osxSign: { identity } } : {}),
    ...(identity && notarize ? { osxNotarize: notarize } : {}),
  },
  hooks: {
    // Before `start` and `package` alike, so the engine is never stale.
    generateAssets: async () => {
      await fetchModel();
      execFileSync("cargo", ["build", "--release", "-p", "samplary-engine"], {
        stdio: "inherit",
      });
    },
    // Without a Developer ID, signed as a whole, ad hoc, so a copy on another
    // Mac opens instead of being called damaged.
    postPackage: async (_config, { outputPaths, platform }) => {
      if (platform !== "darwin" || identity) return;
      for (const out of outputPaths) {
        execFileSync("codesign", ["--force", "--deep", "--sign", "-", `${out}/Samplary.app`], {
          stdio: "inherit",
        });
      }
    },
  },
  makers: [new MakerZIP({}, ["darwin"])],
  plugins: [
    new VitePlugin({
      build: [
        { entry: "main/index.ts", config: "vite.main.config.mts", target: "main" },
        { entry: "main/preload.ts", config: "vite.preload.config.mts", target: "preload" },
      ],
      renderer: [{ name: "main_window", config: "vite.renderer.config.mts" }],
    }),
    new FusesPlugin({
      version: FuseVersion.V1,
      [FuseV1Options.RunAsNode]: false,
      [FuseV1Options.EnableCookieEncryption]: true,
      [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
      [FuseV1Options.EnableNodeCliInspectArguments]: false,
      [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
      [FuseV1Options.OnlyLoadAppFromAsar]: true,
    }),
  ],
};

export default config;
