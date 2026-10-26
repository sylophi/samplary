import { defineConfig } from "vite";

export default defineConfig({
  // Updates only for a Developer ID build (forge.config.ts): Squirrel takes
  // an update only when the same team signed it as the app it replaces.
  define: { UPDATES: JSON.stringify(Boolean(process.env.APPLE_SIGNING_IDENTITY)) },
});
