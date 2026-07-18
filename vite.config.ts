import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Vite runs this file in Node even though the frontend type environment is
// browser-only, so importing Node types solely for one environment variable
// would leak an unnecessary dependency into application type checking.
// @ts-expect-error process is supplied by the Vite configuration runtime.
const host = process.env.TAURI_DEV_HOST;

export default defineConfig(async () => ({
  plugins: [react()],

  // Keeping prior output visible preserves Rust diagnostics when the native
  // side fails before Vite can render its own status.
  clearScreen: false,
  server: {
    // Tauri loads this fixed origin, so silently switching ports would leave
    // the native window connected to the wrong server.
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // Rust already owns its rebuild loop; watching the same tree here would
      // trigger duplicate frontend reloads for every native compilation.
      ignored: ["**/src-tauri/**"],
    },
  },
}));
