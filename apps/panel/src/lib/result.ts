/**
 * What the server said, read in one place.
 *
 * The generated client does **not** throw by default: a 403 comes back as
 * `{ data: undefined, error }` and the promise *resolves*. That is how a
 * refused command used to run react-query's `onSuccess`, and how a refused
 * DELETE used to clear the form for a schedule it had not deleted (review R6).
 *
 * So every call goes through one of the three readers here, chosen by what
 * absence means on that route: [`answerOf`] where there is always a body,
 * [`answerOrNone`] where "there is no such thing" is an ordinary answer, and
 * [`confirmDone`] where success carries no body at all.
 *
 * `throwOnError: true` on an individual call is the other correct spelling and
 * is still in use in a few places. What it costs is what these add: it throws
 * the parsed body, which is not an `Error` and carries no status — so there is
 * nothing to tell a 404 from a 503 by, and [`Refusal`] falls back to the
 * caller's own sentence rather than the server's. Prefer a reader here.
 *
 * This file knows nothing about the client's configuration on purpose — it is
 * imported by presentation, and importing `@/lib/api` would point the panel at
 * an origin as a side effect of drawing an error message.
 */

/**
 * The refusal, as an exception, carrying what the server actually said.
 *
 * `status` is the discriminator the callers need and the error body does not
 * carry: `404` is the ordinary absence of a thing, `403` is a role, `409` is a
 * state the command would not fit, `5xx` is the service having a bad day. `0`
 * means nothing answered at all — no response object, so none of those.
 */
export class ApiFailure extends Error {
  /** The HTTP status, or `0` when the request got no answer. */
  readonly status: number;
  /** `ErrorBody::code` — switch on this, never on the prose. */
  readonly code: string;
  /** One line per problem, for the routes that report every problem at once. */
  readonly details: string[];

  constructor(status: number, body: unknown, fallback: string) {
    const shape = body as
      { code?: string; message?: string; details?: string[] } | null | undefined;
    super(shape?.message || fallback);
    this.name = 'ApiFailure';
    this.status = status;
    this.code = shape?.code ?? (status === 0 ? 'unreachable' : 'error');
    this.details = shape?.details ?? [];
  }

  /** The thing this names is not there — the one refusal that is often ordinary. */
  get isAbsent(): boolean {
    return this.status === 404;
  }
}

/**
 * What the generated SDK hands back when it is not asked to throw.
 *
 * `response` is optional in the client's own types, because a request that
 * never left — a refused connection, a bad URL — has no response to report.
 */
export type SdkResult<T> = {
  data?: T;
  error?: unknown;
  response?: Response;
};

function failureOf(result: SdkResult<unknown>, fallback: string): ApiFailure {
  return new ApiFailure(result.response?.status ?? 0, result.error, fallback);
}

/**
 * The body, or the refusal thrown.
 *
 * `fallback` is the sentence somebody reads when the failure carried no message
 * of its own — a proxy's HTML error page, or a connection refused before
 * anybody wrote JSON.
 */
export function answerOf<T>(result: SdkResult<T>, fallback: string): T {
  if (!result.response?.ok || result.data === undefined) throw failureOf(result, fallback);
  return result.data;
}

/**
 * The body, `null` when there is none to have, and the refusal thrown otherwise.
 *
 * For the reads where absence is an ordinary state rather than a failure: most
 * pipelines have no schedule, and a link outlives the run it names. Only a 404
 * is that. Everything else — a service that is down, a session that has
 * expired, a store nobody configured — is a failure, and drawing it as "there
 * is nothing here" tells somebody their data is gone.
 */
export function answerOrNone<T>(result: SdkResult<T>, fallback: string): T | null {
  if (result.response?.status === 404) return null;
  return answerOf(result, fallback);
}

/**
 * That the server did it, for a route whose success carries no body.
 *
 * A successful DELETE is `204`, and the client turns an empty body into `{}` —
 * so "is there data" is not the question here and never was. HTTP success is.
 */
export function confirmDone(result: SdkResult<unknown>, fallback: string): void {
  if (!result.response?.ok) throw failureOf(result, fallback);
}
