/**
 * What the telemetry client does with a disk, on a Node host.
 *
 * Kept apart from `@aiwatcher/sdk`, which also runs in a browser: nothing here
 * is reached before it is called, and a runtime without Node's modules is
 * told so by name rather than failing at import.
 */
import type { CountSpool, EventEnvelope } from './index.js';

interface Files {
  mkdirSync(path: string, options: { recursive: true }): unknown;
  readFileSync(path: string, encoding?: 'utf8'): string | Uint8Array;
  writeFileSync(path: string, data: string): void;
  renameSync(from: string, to: string): void;
  unlinkSync(path: string): void;
  readdirSync(path: string): string[];
  statSync(path: string): { mtimeMs: number };
}

interface Hashing {
  createHash(algorithm: 'sha256'): { update(data: string | Uint8Array): { digest(encoding: 'hex'): string } };
  randomUUID(): string;
}

interface Urls {
  fileURLToPath(url: string | URL): string;
}

function builtin<T>(name: string): T {
  const found = (
    globalThis as { process?: { getBuiltinModule?: (name: string) => unknown } }
  ).process?.getBuiltinModule?.(name);
  if (found === undefined) {
    throw new Error(`@aiwatcher/sdk/node needs Node's ${name}, which this runtime does not have`);
  }
  return found as T;
}

/** How many runs a count says, and -1 for no count at all. */
function runs(event: EventEnvelope | undefined): number {
  const said = event?.data.runs;
  return typeof said === 'number' ? said : -1;
}

/**
 * A spool of counts in a directory: one file per count, replaced by a larger
 * count and removed once one saying as much is delivered — the same layout
 * `aiwatcher_sdk.HttpTransport(spool_dir=…)` keeps.
 */
export function fileSpool(directory: string): CountSpool {
  const files = builtin<Files>('node:fs');
  const hashing = builtin<Hashing>('node:crypto');
  const pathOf = (event: EventEnvelope): string | undefined => {
    if (event.event_type !== 'client.counted') return undefined;
    const named = JSON.stringify([
      event.source.client ?? null,
      event.variant_id ?? null,
      event.data.evaluation_id ?? null,
      event.data.generation_attempt ?? null,
    ]);
    return `${directory}/counted-${hashing.createHash('sha256').update(named).digest('hex').slice(0, 32)}.json`;
  };
  const read = (path: string): EventEnvelope | undefined => {
    try {
      const kept: unknown = JSON.parse(String(files.readFileSync(path, 'utf8')));
      return typeof kept === 'object' && kept !== null ? (kept as EventEnvelope) : undefined;
    } catch {
      return undefined;
    }
  };
  return {
    keep(event) {
      const path = pathOf(event);
      if (path === undefined || runs(event) <= runs(read(path))) return;
      try {
        files.mkdirSync(directory, { recursive: true });
        const written = `${path}.${hashing.randomUUID()}.tmp`;
        files.writeFileSync(written, JSON.stringify(event));
        files.renameSync(written, path);
      } catch (error) {
        console.warn(`[aiwatcher] could not keep a count in ${directory}`, error);
      }
    },
    release(event) {
      const path = pathOf(event);
      if (path === undefined) return;
      const kept = read(path);
      if (kept === undefined || runs(kept) > runs(event)) return;
      try {
        files.unlinkSync(path);
      } catch {
        // Already gone: nothing is kept.
      }
    },
    kept() {
      let names: string[];
      try {
        names = files.readdirSync(directory);
      } catch {
        return [];
      }
      const kept: { at: number; event: EventEnvelope }[] = [];
      for (const name of names.filter((name) => /^counted-[0-9a-f]{32}\.json$/.test(name))) {
        const path = `${directory}/${name}`;
        const event = read(path);
        if (event?.event_type !== 'client.counted') continue;
        try {
          kept.push({ at: files.statSync(path).mtimeMs, event });
        } catch {
          // Removed while listed.
        }
      }
      return kept.sort((one, other) => one.at - other.at).map((held) => held.event);
    },
  };
}

/**
 * The sha256 of a tool's source file, from its module's `import.meta.url`: what
 * a variant pins as the code that answered a tool (`tool_code`), handed to
 * `ToolWitness.call`'s `code` — the whole module, as Python's `tool_code` hashes.
 */
export function toolCode(moduleUrl: string | URL): string {
  const files = builtin<Files>('node:fs');
  const path = builtin<Urls>('node:url').fileURLToPath(moduleUrl);
  return builtin<Hashing>('node:crypto')
    .createHash('sha256')
    .update(files.readFileSync(path) as Uint8Array)
    .digest('hex');
}
