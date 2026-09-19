/**
 * Reaching a marimo — the one a deployment serves, or the one on somebody's
 * own machine.
 *
 * A lab names a notebook; it never says where marimo is (ADR_0034, amended).
 * That is two different answers with two different owners, and this module is
 * the whole of what the panel needs to tell them apart:
 *
 * - **`/lab-marimo` on this origin** is what a deployment serves. Nothing here
 *   configures it and no address is ever fetched from the server — a URL that
 *   came out of stored data and was then opened by a browser is the shape
 *   `AIWATCHER_WORKFLOW_RUNNER_URL` is configuration to avoid. Whoever deploys
 *   points that path at `marimo edit --base-url /lab-marimo`, exactly as
 *   `/ml-pipeline` is pointed at the notebook runtime.
 * - **An address the viewer supplies** is the other. It defaults to their own
 *   loopback on the port the lab's command uses, and it is where an instructor
 *   sharing a read-only `marimo run` goes too. It is typed by a person, kept in
 *   that person's browser, and never sent anywhere.
 *
 * Reachability is asked differently on each side, and the difference is not
 * cosmetic. Same-origin, the proxy answers and the status is readable, so 404
 * and 5xx mean "nothing is behind that path" the way `ml-pipeline.ts` reads
 * them. Cross-origin, marimo sends no CORS headers, so no status can ever be
 * read — but an opaque response still tells us the connection was made, and a
 * rejection tells us it was not. That is exactly the one bit needed.
 */

/** Where a deployment serves the workshop's notebooks, on this origin. */
export const WORKSHOP_BASE = '/lab-marimo';

/** The command a deployment runs behind {@link WORKSHOP_BASE}. */
export const WORKSHOP_COMMAND = `marimo edit --headless --no-token --host 0.0.0.0 --base-url ${WORKSHOP_BASE} <workshop>`;

/** The default place to look for a marimo somebody started themselves. */
export function loopback(port: number): string {
  return `http://127.0.0.1:${port}`;
}

/**
 * An address this panel will open, or `undefined`.
 *
 * Only `http` and `https`, no credentials in the URL, and the trailing slash
 * dropped so one address composes the same way every time. A refusal is
 * silent on purpose: it is a half-typed address, not an error.
 */
export function normalizeBase(raw: string): string | undefined {
  const trimmed = raw.trim();
  if (trimmed === '') return undefined;
  if (trimmed.startsWith('/')) return trimmed.replace(/\/+$/, '');
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    return undefined;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return undefined;
  if (url.username !== '' || url.password !== '') return undefined;
  return `${url.origin}${url.pathname.replace(/\/+$/, '')}`;
}

/**
 * The URL that opens one notebook on a marimo.
 *
 * `?file=` is how marimo's home routes to a file when it was started on a
 * directory. Started on one file it is already that file and ignores this, so
 * the same URL is right either way.
 */
export function notebookUrl(base: string, path: string | undefined): string {
  return path ? `${base}/?file=${encodeURIComponent(path)}` : `${base}/`;
}

/** Whether something is answering there. */
export async function reachable(base: string): Promise<boolean> {
  const sameOrigin = base.startsWith('/');
  try {
    const response = await fetch(`${base}/`, {
      // Cross-origin, marimo sends no CORS headers, so a readable status is
      // not on offer — but a connection that was made resolves and one that
      // was refused rejects, which is the whole question.
      mode: sameOrigin ? 'cors' : 'no-cors',
      cache: 'no-store',
      redirect: 'follow',
    });
    // The statuses `ml-pipeline.ts` reads as "nothing is listening": vite
    // answers 500 for a refused connection and an ingress 502/503. Opaque
    // responses report 0 and are not judged on status at all.
    if (!sameOrigin) return true;
    return ![404, 500, 502, 503, 504].includes(response.status);
  } catch {
    return false;
  }
}
