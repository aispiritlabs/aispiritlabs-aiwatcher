import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'node:path';

/**
 * The panel's own tests, separate from `vite.config.ts` on purpose.
 *
 * The router plugin generates `routeTree.gen.ts` as a side effect of building
 * and Tailwind compiles a stylesheet nothing here asserts on — neither belongs
 * in a run whose job is to prove a component reads the server's answer.
 */
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: { '@': path.resolve(import.meta.dirname, './src') },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
  },
});
