import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';
import { TanStackRouterVite } from '@tanstack/router-plugin/vite';
import path from 'node:path';

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
      '/api': { target: 'http://127.0.0.1:8080', changeOrigin: true, ws: true },
      // The Flow query service. Optional: when nothing is listening the proxy
      // answers 502, which the Query tab reads as "not running" rather than as
      // a failed query.
      '/flow': { target: 'http://127.0.0.1:8081', changeOrigin: true },
    },
  },
});
