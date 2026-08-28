import { defineConfig } from '@hey-api/openapi-ts';

/**
 * The panel's API client is generated from the Rust routes, not written by
 * hand. A field renamed in `aiwatcher-api` becomes a TypeScript compile error
 * here rather than an `undefined` at runtime.
 *
 * Regenerate with `just openapi` from the repo root, which rebuilds
 * `contracts/openapi.json` first. CI runs `just openapi-check`, so a forgotten
 * regeneration is a red build rather than a runtime surprise.
 */
export default defineConfig({
  input: '../../contracts/openapi.json',
  output: {
    path: './src/api/generated',
    postProcess: ['prettier'],
  },
  plugins: [
    // The fetch client is emitted into the output directory rather than
    // imported from a separate package, which keeps the client and the
    // generator versions from drifting apart.
    '@hey-api/client-fetch',
    // Default operation strategy: one exported function per route.
    '@hey-api/sdk',
    { name: '@hey-api/typescript', enums: 'javascript' },
    // No `zod` plugin: everything the generated SDK returns is already typed
    // by TypeScript, so schemas for it would only be validated twice. Runtime
    // validation is needed exactly where codegen cannot reach — the SSE and
    // WebSocket frames — and those are hand-written in src/lib/live.ts.
  ],
});
