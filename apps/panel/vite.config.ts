import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { TanStackRouterVite } from '@tanstack/router-plugin/vite';
import path from 'node:path';

// The query engine — Flow, DataFusion or DuckDB, whichever this deployment runs.
// `AIWATCHER_FLOW_URL` is the name it had while Flow was the only one, read for
// one release.
const queryUrl =
  process.env.AIWATCHER_QUERY_URL ?? process.env.AIWATCHER_FLOW_URL ?? 'http://127.0.0.1:8081';

export default defineConfig({
  plugins: [
    TanStackRouterVite({ target: 'react', autoCodeSplitting: true }),
    react(),
    tailwindcss(),
  ],
  resolve: {
    alias: { '@': path.resolve(import.meta.dirname, './src') },
  },
  server: {
    port: 5173,
    proxy: {
      // Same-origin in development, so the API needs no CORS configuration and
      // SSE/WebSocket behave exactly as they will in production behind one
      // ingress.
      '/api': {
        target: process.env.AIWATCHER_API_URL ?? 'http://127.0.0.1:8080',
        changeOrigin: true,
        ws: true,
      },
      // The query engine. Optional: when nothing is listening the proxy answers
      // 502, which the Query tab reads as "not running" rather than as a failed
      // query. With the trailing slash, because a prefix match on `/query` alone
      // would also take any panel path that happened to start with it.
      '/query/': { target: queryUrl, changeOrigin: true },
      // The prefix the engine had while Flow was the only one, proxied to the
      // same place for one release.
      '/flow/': { target: queryUrl, changeOrigin: true },
      // The `ml_pipeline` notebook runtime, optional in exactly the same way.
      // `ws` is not optional though: a marimo app talks to its kernel over a
      // WebSocket, so without it the block's editor loads, renders nothing and
      // never says why.
      '/ml-pipeline': {
        target: process.env.AIWATCHER_ML_PIPELINE_URL ?? 'http://127.0.0.1:8082',
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
