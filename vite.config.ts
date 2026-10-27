import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react()],

  // The manifold-3d worker (src/ops/manifold.worker.ts) is an ES module worker: manifold.js uses
  // top-level await and import.meta.url, which the default "iife" worker format cannot bundle.
  worker: { format: "es" as const },
  // Dev only. The dependency scan does not follow `new Worker(new URL(...))`, so pre-bundling would find
  // manifold-3d only when the worker first imports it, then re-optimise and reload the page mid-operation.
  // manifold.js is already a single ES module, so it is served as is.
  optimizeDeps: { exclude: ["manifold-3d"] },

  build: {
    rolldownOptions: {
      output: {
        // The libraries in files of their own: the webview parses and compiles separate scripts in parallel, and the
        // dialogs and mode panels that load on demand (DialogHost, RightPanel) share them instead of copying them.
        // three.js is split by size too, so no file passes 500 kB.
        codeSplitting: {
          groups: [
            { name: "three", test: /[\\/]node_modules[\\/]three[\\/]/, priority: 3, maxSize: 400_000 },
            { name: "react", test: /[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/, priority: 2 },
            { name: "vendor", test: /[\\/]node_modules[\\/]/, priority: 1 },
          ],
        },
      },
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
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
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
