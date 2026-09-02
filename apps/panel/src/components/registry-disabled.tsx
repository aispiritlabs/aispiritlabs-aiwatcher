import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/primitives';

/**
 * What an authored-artifact area shows when the instance has no object store.
 *
 * The API answers 501 rather than 404 for this, and the difference is the
 * whole reason this component exists: an empty list would say "nothing here
 * yet", which is a different problem with a different fix. This says which
 * variable is unset.
 *
 * One component for three areas, because one setting decides all three:
 * prompts, curated datasets and annotations share the configured store under
 * separate key prefixes. `area` is only the heading — the fix is identical.
 *
 * The conversation archive answers the same 501 with a *different* fix, which
 * is why it has its own component below rather than a fourth bullet here: it
 * needs the store **and** a decision to keep conversation content **and** a
 * key, and a reader sent to `AIWATCHER_PROMPT_STORE` would set the one
 * variable that was already right.
 */
export function isRegistryDisabled(error: unknown): boolean {
  const body = error as { code?: string } | null | undefined;
  return body?.code === 'registry_disabled';
}

export function RegistryDisabled({ area = 'Prompts' }: { area?: string }) {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">{area}</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          This instance is running without an authored-artifact store.
        </p>
      </div>
      <Card className="border-dashed">
        <CardHeader>
          <CardTitle className="text-sm">No store is configured</CardTitle>
        </CardHeader>
        <CardContent className="max-w-3xl text-xs leading-relaxed text-muted-foreground">
          Prompts, curated datasets and annotations are the things here that outlive retention, so
          they need somewhere durable to write — and one setting decides all three. Set{' '}
          <code>AIWATCHER_PROMPT_STORE</code>:
          <ul className="mt-2 flex list-inside list-disc flex-col gap-1">
            <li>
              <code>file</code> — a directory under <code>AIWATCHER_DATA_DIR</code>. The default,
              and what <code>just run</code> uses.
            </li>
            <li>
              <code>s3</code> — an S3 endpoint, with <code>AIWATCHER_PROMPT_S3_ENDPOINT</code>,{' '}
              <code>…_ACCESS_KEY</code> and <code>…_SECRET_KEY</code>. RustFS in the compose stack;
              MinIO, Ceph or AWS work unchanged.
            </li>
            <li>
              <code>memory</code> — nothing survives a restart. For demos.
            </li>
          </ul>
        </CardContent>
      </Card>
    </div>
  );
}

/**
 * What the conversation area shows when this instance keeps no archive.
 *
 * Off is the default and the right default: a deployment that has decided
 * nothing about how it governs conversation content must not already be
 * holding some. So this reads as a decision to make rather than a
 * misconfiguration to correct — which is also why it says what turning it on
 * commits somebody to.
 */
export function ArchiveDisabled() {
  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Conversations</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          This instance retains no conversation content.
        </p>
      </div>
      <Card className="border-dashed">
        <CardHeader>
          <CardTitle className="text-sm">The archive is off, which is the default</CardTitle>
        </CardHeader>
        <CardContent className="max-w-3xl text-xs leading-relaxed text-muted-foreground">
          Traces keep operational fields — model, tokens, latency, outcome — and never the words.
          Keeping the words is a separate decision, with its own encryption key and its own
          retention clock, and it is the one thing here that has to be switched on deliberately.
          <ul className="mt-2 flex list-inside list-disc flex-col gap-1">
            <li>
              <code>AIWATCHER_CONVERSATION_ARCHIVE=on</code> — turns the routes on.
            </li>
            <li>
              <code>AIWATCHER_CONVERSATION_KEYS=id:key</code> — 32 base64url bytes, active key
              first. The server refuses to start without one, because an archive with no key is a
              plaintext archive.
            </li>
            <li>
              <code>AIWATCHER_CONVERSATION_POLICY</code> — <code>protected</code> (the default)
              refuses a turn with no consent record; <code>open</code> records the gap instead, and
              every export then excludes those turns by name.
            </li>
          </ul>
          It also needs <code>AIWATCHER_PROMPT_STORE</code>, which the other authored areas already
          use. See ADR_0021.
        </CardContent>
      </Card>
    </div>
  );
}
