import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";

export default defineConfig(({ mode }) => {
  // This checkout's own port (port-pool), so checkouts side by side don't collide.
  const env = loadEnv(mode, import.meta.dirname, "");
  return {
    plugins: [tailwindcss(), react()],
    server: { port: env.PORT ? Number(env.PORT) : undefined, strictPort: Boolean(env.PORT) },
    resolve: { alias: { "@": resolve(import.meta.dirname, "renderer") } },
  };
});
