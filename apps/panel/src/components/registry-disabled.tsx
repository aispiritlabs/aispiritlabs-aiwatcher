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
