import type { Client } from '@/api/generated/client';

/**
 * Which project the panel is reading, and which reads can answer for one.
 *
 * The scope is **not a filter**. Filters narrow rows a reader may already see
 * and live in each view's own URL contract; a scope decides which rows exist
 * at all, and the server takes a fresh grant decision on every request that
 * carries one (`ProjectAccess` has an `evaluated_at`, ADR_0033). So it rides
 * one level above every filter: one search parameter on the root route,
 * retained across every navigation, and one rewrite here in the transport.
 *
 * **One seam, because ninety-seven files call the generated client.** A scope
 * applied per call is a scope somebody forgets, and the read that forgets is
 * the one that answers instance-wide under a header naming a project — which
 * is precisely the lie the panel spent a year refusing to tell. So the
 * rewrite happens once, on the route template the generated SDK passes, and no
 * feature names a project to get its own data.
 *
 * **And it reaches exactly as far as the data plane does.** [`SCOPED_ROUTES`]
 * is the set of instance routes that have a scoped twin, taken from the
 * contract rather than believed — a request on any other route is left alone,
 * because a rewrite to a route that does not exist is a 404 that reads as "you
 * may not see this" when the truth is "this is not a project's to see".
 */

export interface ProjectScope {
  organization: string;
  project: string;
}

/** `<organization>/<project>`, the spelling the log and the ingest token use. */
export function formatScope(scope: ProjectScope): string {
  return `${scope.organization}/${scope.project}`;
}

/**
 * A scope from the URL, or `undefined` for the unassigned side.
 *
 * Deliberately not validated against the grants here: whether the caller may
 * open it is the server's answer on the next request, and a browser that
 * decided it itself would be a second copy of the policy.
 */
export function parseScope(value: string | undefined): ProjectScope | undefined {
  if (!value) return undefined;
  const [organization, project, ...rest] = value.split('/');
  if (!organization || !project || rest.length > 0) return undefined;
  return { organization, project };
}

/**
 * Every instance route that has a project twin, as the contract spells it.
 *
 * Checked against `contracts/openapi.json` by `scope.test.ts` rather than
 * trusted, and that test is the one that fails on the day the workflow or
 * evaluation folds gain a project — which is the day this list grows and an
 * area stops being instance-wide. Regenerate with:
 *
 *     python3 -c "import json;d=json.load(open('contracts/openapi.json'));\
 *     P='/api/v1/orgs/{organization}/projects/{project}/';p=set(d['paths']);\
 *     [print(f\"  '{t}',\") for t in sorted({'/api/v1/'+q[len(P):] for q in p if q.startswith(P)} & p)]"
 */
export const SCOPED_ROUTES: ReadonlySet<string> = new Set([
  '/api/v1/annotation-blobs',
  '/api/v1/annotation-blobs/{image_id}',
  '/api/v1/annotation-export',
  '/api/v1/annotation-export/coco',
  '/api/v1/annotation-exports',
  '/api/v1/annotation-image',
  '/api/v1/annotation-images',
  '/api/v1/annotation-project',
  '/api/v1/annotation-projects',
  '/api/v1/annotation-reviews',
  '/api/v1/annotation-revisions',
  '/api/v1/conversations',
  '/api/v1/curation-library',
  '/api/v1/curation-pipelines',
  '/api/v1/curation-pipelines/{name}/revisions/{revision}',
  '/api/v1/curations',
  '/api/v1/dataset-rows',
  '/api/v1/dataset-samples',
  '/api/v1/datasets',
  '/api/v1/dimensions/{kind}',
  '/api/v1/evaluation-approvals',
  '/api/v1/evaluation-approvals/{approval_id}',
  '/api/v1/evaluation-approvals/{approval_id}/bundle',
  '/api/v1/evaluation-approvals/{approval_id}/bundle/{name}',
  '/api/v1/evaluation-assessments',
  '/api/v1/evaluation-assessments/{target_id}/{standing_id}',
  '/api/v1/evaluation-calibrations',
  '/api/v1/evaluation-calibrations/{version}',
  '/api/v1/evaluation-cohorts',
  '/api/v1/evaluation-cohorts/{cases}',
  '/api/v1/evaluation-recordings/{digest}/content',
  '/api/v1/evaluation-recordings/{name}',
  '/api/v1/evaluation-results',
  '/api/v1/evaluation-results/{evaluation_id}',
  '/api/v1/evaluation-results/{evaluation_id}/cases',
  '/api/v1/evaluation-results/{evaluation_id}/comparison',
  '/api/v1/evaluation-results/{evaluation_id}/comparison/cases',
  '/api/v1/evaluation-reviews',
  '/api/v1/evaluation-reviews/of-target',
  '/api/v1/evaluation-reviews/publish',
  '/api/v1/evaluation-reviews/{id}/actions',
  '/api/v1/evaluation-rubrics',
  '/api/v1/evaluation-rubrics/{name}',
  '/api/v1/evaluation-runs',
  '/api/v1/evaluation-runs/{id}',
  '/api/v1/evaluation-runs/{id}/start',
  '/api/v1/evaluation-scorecards',
  '/api/v1/evaluation-scorecards/{name}',
  '/api/v1/evaluation-scorecards/{name}/diff',
  '/api/v1/evaluation-scorecards/{name}/versions',
  '/api/v1/evaluation-suites',
  '/api/v1/evaluations',
  '/api/v1/evaluations/{evaluation_id}',
  '/api/v1/events/stream',
  '/api/v1/executions/{execution_id}',
  '/api/v1/executions/{execution_id}/commands/cancel',
  '/api/v1/executions/{execution_id}/commands/pause',
  '/api/v1/executions/{execution_id}/commands/resume',
  '/api/v1/executions/{execution_id}/history',
  '/api/v1/executions/{execution_id}/steps/{step_id}/commands/retry',
  '/api/v1/executions/{execution_id}/steps/{step_id}/input',
  '/api/v1/experiments',
  '/api/v1/experiments/{context_id}',
  '/api/v1/labs',
  '/api/v1/labs/{name}',
  '/api/v1/labs/{name}/labels/{label}',
  '/api/v1/labs/{name}/measurement',
  '/api/v1/labs/{name}/versions/{version_id}',
  '/api/v1/live',
  '/api/v1/metrics',
  '/api/v1/models',
  '/api/v1/models/{name}',
  '/api/v1/models/{name}/labels',
  '/api/v1/prompts',
  '/api/v1/prompts/{name}',
  '/api/v1/prompts/{name}/labels/{label}',
  '/api/v1/prompts/{name}/optimizations',
  '/api/v1/prompts/{name}/optimizations/{optimization_id}',
  '/api/v1/prompts/{name}/rebuild',
  '/api/v1/prompts/{name}/versions/{version_id}',
  '/api/v1/runs',
  '/api/v1/runs/{run_id}',
  '/api/v1/runs/{run_id}/events',
  '/api/v1/runs/{run_id}/stream',
  '/api/v1/spans',
  '/api/v1/training-runs',
  '/api/v1/training-runs/{run_id}',
  '/api/v1/training-runs/{run_id}/finish',
  '/api/v1/training-runs/{run_id}/progress',
  '/api/v1/workflow-definitions',
  '/api/v1/workflow-definitions/{name}',
  '/api/v1/workflow-executions',
  '/api/v1/workflow-executions/{workflow_run_id}',
  '/api/v1/workflow-executions/{workflow_run_id}/stream',
  '/api/v1/workflows',
  '/api/v1/workflows/{workflow_id}',
]);

const PREFIX = '/api/v1/';

/**
 * The scope every read runs under, or `undefined` for the unassigned side.
 *
 * A module variable rather than React state, and set from the URL in the root
 * route's `beforeLoad` — which runs before every loader and before any render,
 * so a deep link carrying a scope cannot issue one instance-wide read first.
 * The URL is still the only source: nothing writes this except that hook, and
 * nothing reads the panel's scope from anywhere else.
 */
let active: ProjectScope | undefined;

export function activeScope(): ProjectScope | undefined {
  return active;
}

export function setActiveScope(scope: ProjectScope | undefined): void {
  active = scope;
}

/** Whether this route template can answer for one project. */
export function isScopedRoute(template: string): boolean {
  return SCOPED_ROUTES.has(template);
}

/**
 * A hand-built path under a scope, or unchanged when no twin exists.
 *
 * `live.ts` builds its own URLs because `EventSource` and `WebSocket` are not
 * the generated client, so the same decision has to be reachable from there.
 * It takes the template as well as the path: `/api/v1/runs/abc/stream` is not
 * in the contract, `/api/v1/runs/{run_id}/stream` is.
 */
export function scopedPath(
  path: string,
  template: string,
  scope: ProjectScope | undefined = active,
): string {
  if (!scope || !isScopedRoute(template) || !path.startsWith(PREFIX)) return path;
  return `${PREFIX}orgs/${encodeURIComponent(scope.organization)}/projects/${encodeURIComponent(
    scope.project,
  )}/${path.slice(PREFIX.length)}`;
}

/**
 * Point the generated client at the selected project, once.
 *
 * Two things ride with the rewrite. `X-AIWatcher-IAM` because a scoped **write**
 * is refused without it — it is not a simple header, so a cross-origin form
 * carrying a session cookie cannot set one — and it is harmless on a read, so
 * adding it here is cheaper than teaching every mutation about scopes. And a
 * development warning when a page whose area is declared a project's makes a
 * read that cannot be one, because that is the drift this design has to notice:
 * the area's reach is written down in `navigation.ts` and the contract is what
 * decides, and the two must not disagree quietly.
 */
export function installScope(client: Client): void {
  client.interceptors.request.use(
    (request: Request, options: { url?: string; serializedBody?: string }) => {
      const scope = active;
      const template = options.url;
      if (!scope || !template) return request;
      if (!isScopedRoute(template)) {
        if (import.meta.env.DEV) reportInstanceWide(template);
        return request;
      }
      const url = new URL(request.url);
      url.pathname = scopedPath(url.pathname, template, scope);
      // Rebuilt from its own fields rather than from itself. `new Request(url,
      // request)` is the obvious spelling and it carries the original's
      // `AbortSignal` across, which the test environment's fetch refuses as
      // foreign — and nothing here passes a signal in the first place, because
      // every call is a react-query `queryFn` that does not forward one.
      const scoped = new Request(url, {
        method: request.method,
        headers: request.headers,
        body: options.serializedBody ?? null,
        credentials: request.credentials,
        redirect: request.redirect,
      });
      scoped.headers.set('X-AIWatcher-IAM', '1');
      return scoped;
    },
  );
}

/**
 * Say so, in development, when a project is selected and a read answered for
 * the instance anyway.
 *
 * Not an error and not a toast: on an area the navigation already declares
 * instance-wide this is the truth and the page says it in words. What this
 * catches is the other case — a route added to a project-scoped area without a
 * scoped twin, which no type and no test would notice until somebody read one
 * project's screen and saw another's rows.
 */
const warned = new Set<string>();

/**
 * What a project could never scope, so a warning about it says nothing.
 *
 * Signing in, the control plane and the deployment's own inventory are about
 * the instance by definition — `/iam` is where a project is *administered*,
 * and a scoped twin of it would be a project deciding who may see itself.
 */
const NEVER_SCOPED = ['/api/v1/auth/', '/api/v1/iam/', '/api/v1/system'];

function reportInstanceWide(template: string): void {
  if (NEVER_SCOPED.some((prefix) => template.startsWith(prefix))) return;
  if (warned.has(template)) return;
  warned.add(template);
  console.info(
    `[aiwatcher] ${template} has no project twin: answered instance-wide while a project is selected.`,
  );
}
