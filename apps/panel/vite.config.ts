import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { TanStackRouterVite } from '@tanstack/router-plugin/vite';
import path from 'node:path';

// The query engine — Flow, DataFusion or DuckDB, whichever this deployment runs.
// `AIWATCHER_FLOW_URL` was its name while Flow was the only one; refused rather
// than ignored, because ignoring it would quietly proxy to the default port.
if (process.env.AIWATCHER_FLOW_URL) {
  throw new Error(
    'AIWATCHER_FLOW_URL is no longer read; set AIWATCHER_QUERY_URL to the query engine instead.',
  );
}
const queryUrl = process.env.AIWATCHER_QUERY_URL ?? 'http://127.0.0.1:8081';

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
      // The `ml_pipeline` notebook runtime, optional in exactly the same way.
      // `ws` is not optional though: a marimo app talks to its kernel over a
      // WebSocket, so without it the block's editor loads, renders nothing and
      // never says why.
      '/ml-pipeline': {
        target: process.env.AIWATCHER_ML_PIPELINE_URL ?? 'http://127.0.0.1:8082',
        changeOrigin: true,
        ws: true,
      },
      // A marimo serving a workshop's own notebooks, for the labs that hand one
      // out (ADR_0034, amended). Optional in the same way, and the only reason
      // it is a path on this origin rather than an address the panel is told is
      // that an address out of stored data, opened by a browser, is the shape
      // `AIWATCHER_WORKFLOW_RUNNER_URL` is configuration to avoid.
      //
      // Whatever serves it runs with `--base-url /lab-marimo`, so marimo emits
      // its own URLs under the prefix and nothing is rewritten here. `ws` for
      // the same reason as above: without it the notebook loads, renders
      // nothing, and never says why.
      '/lab-marimo': {
        target: process.env.AIWATCHER_LAB_MARIMO_URL ?? 'http://127.0.0.1:2718',
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
