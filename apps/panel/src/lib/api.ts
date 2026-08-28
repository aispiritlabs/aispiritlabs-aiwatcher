import { client } from '@/api/generated/client.gen';

/**
 * The generated client points at the same origin. In development Vite proxies
 * `/api` to the Rust server, so SSE and WebSocket behave exactly as they will
 * behind a single ingress in production — no CORS to configure and no
 * environment-specific base URL to get wrong.
 */
client.setConfig({
  baseUrl: import.meta.env.VITE_API_BASE_URL ?? '',
});

export { client };
