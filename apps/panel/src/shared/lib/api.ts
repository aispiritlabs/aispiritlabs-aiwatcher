import { client } from '@/api/generated/client.gen';
import { installScope } from '@/shared/lib/scope';

/**
 * The generated client points at the same origin. In development Vite proxies
 * `/api` to the Rust server, so SSE and WebSocket behave exactly as they will
 * behind a single ingress in production — no CORS to configure and no
 * environment-specific base URL to get wrong.
 */
client.setConfig({
  baseUrl: import.meta.env.VITE_API_BASE_URL ?? '',
});

/**
 * And at the project the panel is reading, if one is selected.
 *
 * Here rather than at each call site: the scope is one level above every
 * read, and a read that has to remember to carry it is a read that will
 * answer for the whole instance under a header naming one project.
 */
installScope(client);

export { client };
