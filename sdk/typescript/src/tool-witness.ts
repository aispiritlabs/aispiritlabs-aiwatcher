/**
 * A tool's calls, witnessed where the tool runs, by keyed digests.
 *
 * The same witness the Python gateway relays a deployment's tools through
 * (`aiwatcher_sdk.gateway.ToolWitness`), for a tool whose host is a
 * JavaScript process: it publishes a run naming the caller's run
 * (`CALLER_RUN_HEADER`) holding digests of each part of the arguments and of
 * the bytes the host sends back — and nothing said in either. The digests are
 * `aiwatcher_core::witness`'s, byte for byte, made under a witness key: the
 * gateway's (`aiwatcher-gateway --witness-key`), held by a host publishing
 * under a token of its own that the deployment names a witness digesting under
 * the gateway's key (`AIWATCHER_WITNESS_DIGESTS`) — or derived from the
 * gateway's own token. Only digests under one key say that a value a call the
 * gateway relayed was rendered with is what this tool returned.
 *
 *     const witness = ToolWitness.withKey(telemetry, gatewayWitnessKey);
 *     await witness.call('atlas', args, { caller: request.headers[CALLER_RUN_HEADER] },
 *       async (call) => call.answered(JSON.stringify(await lookUp(args))));
 *
 * A tool an application would compute in its own process is accountable here,
 * on a host with the key, or behind the gateway, whose `tools` may answer with a
 * function a variant pins by its source's sha256 (`tool_code`).
 */
import type { AiwatcherClient } from './index.js';

/** What a witness key is derived for — `aiwatcher_core::witness`, byte for byte. */
export const WITNESS_KEY_LABEL = 'aiwatcher.witness.v1';
/** How many digests of one side of a call are published. */
export const MOST_DIGESTS = 64;
/** What is returned past this many bytes is relayed and not digested. */
export const MAX_BODY_BYTES = 8 * 1024 * 1024;
/**
 * The characters Rust's `str::trim` removes: Unicode's White_Space, which is
 * neither what `String.prototype.trim` removes nor what Python's `strip` does.
 */
const WHITE_SPACE = new Set(
  '\t\n\u000b\u000c\r \u0085\u00a0\u1680\u2000\u2001\u2002\u2003\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000',
);

export type Said = 'asked' | 'replied' | 'taking';

const encoder = new TextEncoder();

async function hmac(key: Uint8Array, message: Uint8Array): Promise<Uint8Array> {
  const imported = await globalThis.crypto.subtle.importKey(
    'raw',
    key as Uint8Array<ArrayBuffer>,
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign'],
  );
  return new Uint8Array(
    await globalThis.crypto.subtle.sign('HMAC', imported, message as Uint8Array<ArrayBuffer>),
  );
}

const hex = (bytes: Uint8Array) =>
  [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('');

/** A text with Unicode's White_Space removed from both ends, as Rust trims it. */
export function trimmed(text: string): string {
  const characters = [...text];
  let from = 0;
  let to = characters.length;
  while (from < to && WHITE_SPACE.has(characters[from] ?? '')) from++;
  while (to > from && WHITE_SPACE.has(characters[to - 1] ?? '')) to--;
  return characters.slice(from, to).join('');
}

/**
 * A text as a question asked in other words still reads —
 * `aiwatcher_core::witness::normalized`: NFKC, lower case, every punctuation
 * character (general category P) gone, and each run of white space one space,
 * none at either end. Byte for byte what the deployment and the Python gateway
 * compute, for every character this runtime's Unicode version assigns.
 */
export function normalized(text: string): string {
  let out = '';
  let space = false;
  for (const character of text.normalize('NFKC').toLowerCase()) {
    if (/\p{P}/u.test(character)) continue;
    if (WHITE_SPACE.has(character)) {
      space = out.length > 0;
      continue;
    }
    if (space) {
      out += ' ';
      space = false;
    }
    out += character;
  }
  return out;
}

/** The witness key of a credential, from its secret. */
export async function witnessKey(secret: string): Promise<Uint8Array> {
  return hmac(encoder.encode(secret), encoder.encode(WITNESS_KEY_LABEL));
}

/** The digest of one text on one side of a call, trimmed of surrounding white space first. */
export async function witnessDigest(key: Uint8Array, said: Said, text: string): Promise<string> {
  const message = encoder.encode(`${said}\u0000${trimmed(text)}`);
  return hex(await hmac(key, message)).slice(0, 32);
}

/**
 * A number as `aiwatcher_core::witness::number` spells it, which is how
 * JavaScript's `String(number)` does — and a `bigint` digit for digit. A
 * JavaScript number holds an integer wider than 2^53 only as the double nearest
 * it, and is spelled as that double.
 */
export function canonicalNumber(value: number | bigint): string {
  if (typeof value === 'bigint') return value.toString();
  if (!Number.isFinite(value) || value === 0) return '0';
  return String(value);
}

/** Compares two texts by code point, as Rust orders a `String`. */
function byCodePoint(a: string, b: string): number {
  const left = [...a];
  const right = [...b];
  for (let at = 0; at < Math.min(left.length, right.length); at++) {
    const difference = (left[at]?.codePointAt(0) ?? 0) - (right[at]?.codePointAt(0) ?? 0);
    if (difference !== 0) return difference;
  }
  return left.length - right.length;
}

/**
 * A JSON value as one text, byte for byte as `aiwatcher_core::witness::canonical`
 * writes it: keys sorted by code point, nothing between tokens, JSON's string
 * escapes, and every number as {@link canonicalNumber} spells it.
 */
export function canonical(value: unknown): string {
  if (value === null || value === undefined) return 'null';
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'number' || typeof value === 'bigint') return canonicalNumber(value);
  if (typeof value === 'string') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (typeof value === 'object') {
    const fields = value as Record<string, unknown>;
    return `{${Object.keys(fields)
      .sort(byCodePoint)
      .map((key) => `${JSON.stringify(key)}:${canonical(fields[key])}`)
      .join(',')}}`;
  }
  return JSON.stringify(String(value));
}

/** A value as text: itself where it is text, its canonical JSON where it is not. */
const asText = (value: unknown) => (typeof value === 'string' ? value : canonical(value));

/**
 * Each non-blank text inside a JSON value, and each number and flag as its
 * canonical JSON — what a value's parts are compared as.
 */
export function leaves(value: unknown): string[] {
  if (value === null || value === undefined) return [];
  if (Array.isArray(value)) return value.flatMap(leaves);
  if (typeof value === 'object') return Object.values(value as Record<string, unknown>).flatMap(leaves);
  const text = asText(value);
  return trimmed(text) ? [text] : [];
}

async function digested(key: Uint8Array, said: Said, texts: string[]): Promise<string[]> {
  const digests: string[] = [];
  for (const text of texts) {
    const digest = await witnessDigest(key, said, text);
    if (!digests.includes(digest)) digests.push(digest);
    if (digests.length === MOST_DIGESTS) break;
  }
  return digests;
}

/** What a tool answered one call with, as its host sends it back. */
export class ToolCall {
  returned: Uint8Array = new Uint8Array();
  status = 200;

  /** The bytes the host sends the caller, exactly: what a later request renders. */
  answered(returned: Uint8Array | string, status = 200): void {
    this.returned = typeof returned === 'string' ? encoder.encode(returned) : returned;
    this.status = status;
  }
}

export interface ToolReport {
  caller?: string | undefined;
  name: string;
  arguments: unknown;
  returned: Uint8Array;
  status: number;
  /** When the call started, from `performance.now()`. */
  startedMs: number;
}

export class ToolWitness {
  readonly #telemetry: AiwatcherClient;
  readonly #key: Uint8Array | undefined;

  /** A witness digesting under `key`; with none, it publishes no digests. */
  constructor(telemetry: AiwatcherClient, key?: Uint8Array) {
    this.#telemetry = telemetry;
    this.#key = key;
  }

  /** A witness digesting under the gateway's witness key, handed over as hex. */
  static withKey(telemetry: AiwatcherClient, hexKey: string): ToolWitness {
    const pairs = hexKey.match(/[0-9a-f]{2}/gi) ?? [];
    return new ToolWitness(telemetry, new Uint8Array(pairs.map((pair) => Number.parseInt(pair, 16))));
  }

  /** A witness digesting under the key derived from a credential's secret. */
  static async withCredential(telemetry: AiwatcherClient, secret: string): Promise<ToolWitness> {
    return new ToolWitness(telemetry, await witnessKey(secret));
  }

  /**
   * One call of the tool `name` with `args`, reported once it is answered. A
   * body that throws is reported as a failed call with nothing returned, and
   * the error goes on.
   */
  async call<T>(
    name: string,
    args: unknown,
    options: { caller?: string | undefined },
    body: (call: ToolCall) => Promise<T>,
  ): Promise<T> {
    const startedMs = performance.now();
    const answer = new ToolCall();
    let result: T;
    try {
      result = await body(answer);
    } catch (error) {
      await this.report({
        caller: options.caller,
        name,
        arguments: args,
        returned: new Uint8Array(),
        status: 500,
        startedMs,
      });
      throw error;
    }
    await this.report({
      caller: options.caller,
      name,
      arguments: args,
      returned: answer.returned,
      status: answer.status,
      startedMs,
    });
    return result;
  }

  /**
   * One tool call: keyed digests of each part of its arguments and of what it
   * returned, and nothing said in either. Never throws: a witness that could
   * not report must not take the tool down.
   */
  async report(report: ToolReport): Promise<void> {
    try {
      const outcome: Record<string, unknown> = {
        status_code: report.status,
        outcome: report.status < 400 ? 'succeeded' : 'failed',
      };
      if (this.#key) {
        outcome.arguments_digests = await digested(this.#key, 'replied', leaves(report.arguments));
        const texts: string[] = [];
        if (report.returned.length <= MAX_BODY_BYTES && report.status < 400) {
          const text = new TextDecoder().decode(report.returned);
          texts.push(text);
          try {
            texts.push(canonical(JSON.parse(text)));
          } catch {
            // Not JSON: the text alone is what a later request renders.
          }
        }
        outcome.returned_digests = await digested(this.#key, 'replied', texts);
      }
      const runId = `gateway-${globalThis.crypto.randomUUID()}`;
      const correlationId = globalThis.crypto.randomUUID();
      await this.#telemetry.run(
        runId,
        { correlationId, ...(report.caller ? { callerRunId: report.caller } : {}) },
        async (run) =>
          run.agent('gateway', async () => {
            // Its start and its end together, once the tool has answered: what
            // it returned is only known then, and the start would say nothing.
            const context = { runId, correlationId, agentId: 'gateway', causationId: correlationId };
            const call = { call_id: globalThis.crypto.randomUUID(), tool_name: report.name };
            this.#telemetry.emit('tool.started', context, call);
            this.#telemetry.emit('tool.completed', context, {
              ...call,
              ...outcome,
              duration_ms: Math.round((performance.now() - report.startedMs) * 1000) / 1000,
            });
          }),
      );
    } catch {
      // Telemetry never takes the tool down.
    }
  }
}
