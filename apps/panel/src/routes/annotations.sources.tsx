import { createFileRoute } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { ExternalLink } from 'lucide-react';
import { z } from 'zod';

import { listSources } from '@/api/generated';
import type { SourceUsage } from '@/api/generated/types.gen';
import { Badge, Card, CardContent, CardHeader, CardTitle, Spinner } from '@/components/ui/primitives';

/**
 * Where the images come from.
 *
 * A dated table an instance was configured with, not a search against Hugging
 * Face, Kaggle or Roboflow Universe. Those mirrors restate licences wrongly
 * often enough that a live answer would be worse than none: it would arrive
 * looking authoritative. Every row links its original and says when somebody
 * last read the licence there.
 *
 * This build ships no rows — which corpora exist and what their licences
 * permit is a question about one field, and a list shipped here would be one
 * project's homework. Empty is a working state: nothing outranks a mirror's
 * claim, so every hub result stays `unclear`.
 *
 * The filter that matters is the first one. "What may a commercial model be
 * trained on" is a question with an expensive wrong answer, and it should be
 * one click rather than an afternoon of reading licence files.
 */

const searchSchema = z.object({
  q: z.string().optional(),
  usage: z.enum(['commercial', 'non_commercial', 'unclear']).optional(),
  label: z.string().optional(),
  project: z.string().optional(),
});

export const Route = createFileRoute('/annotations/sources')({
  validateSearch: searchSchema,
  component: SourcesPage,
});

const USAGE_TONES: Record<SourceUsage, 'success' | 'danger' | 'warning'> = {
  commercial: 'success',
  non_commercial: 'danger',
  unclear: 'warning',
};

const LABELS = ['walls', 'rooms', 'doors', 'windows', 'openings', 'text', 'scale', 'graph'];

function SourcesPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();

  const sources = useQuery({
    queryKey: ['annotation-sources', search.q, search.usage, search.label],
    queryFn: async () => {
      const response = await listSources({
        throwOnError: true,
        query: { q: search.q || undefined, usage: search.usage, label: search.label },
      });
      return response.data;
    },
  });

  const update = (next: Partial<z.infer<typeof searchSchema>>) =>
    navigate({ search: (previous) => ({ ...previous, ...next }) });

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Dataset sources</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          The corpora somebody read the licence of, what each of them labels, and what it permits.
          Dated and linked rather than fetched: a mirror&rsquo;s declared licence is often not the
          original&rsquo;s, and this table is a signpost, never a permission.
        </p>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <input
          defaultValue={search.q ?? ''}
          onChange={(event) => update({ q: event.target.value || undefined })}
          placeholder="name, licence, what it labels…"
          className="w-64 rounded-md border border-border bg-background px-2 py-1.5 text-sm"
        />
        <select
          value={search.usage ?? ''}
          onChange={(event) =>
            update({ usage: (event.target.value || undefined) as SourceUsage | undefined })
          }
          className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
        >
          <option value="">any licence</option>
          <option value="commercial">commercial use permitted</option>
          <option value="non_commercial">research only</option>
          <option value="unclear">unclear</option>
        </select>
        <select
          value={search.label ?? ''}
          onChange={(event) => update({ label: event.target.value || undefined })}
          className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
        >
          <option value="">any labels</option>
          {LABELS.map((label) => (
            <option key={label} value={label}>
              labels {label}
            </option>
          ))}
        </select>
        {sources.data && (
          <span className="text-xs text-muted-foreground">
            {sources.data.sources.length} of {sources.data.total}
          </span>
        )}
      </div>

      {sources.isLoading ? (
        <div className="flex justify-center p-10">
          <Spinner />
        </div>
      ) : (
        <div className="grid gap-3 lg:grid-cols-2">
          {(sources.data?.sources ?? []).map((source) => (
            <Card key={source.id}>
              <CardHeader className="flex-row items-start justify-between gap-2">
                <div>
                  <CardTitle className="text-sm">
                    <a
                      href={source.url}
                      target="_blank"
                      rel="noreferrer noopener"
                      className="inline-flex items-center gap-1 hover:underline"
                    >
                      {source.name}
                      <ExternalLink className="h-3 w-3" />
                    </a>
                  </CardTitle>
                  <p className="mt-1 text-xs text-muted-foreground">{source.summary}</p>
                </div>
                <Badge tone={USAGE_TONES[source.usage ?? 'unclear']} className="shrink-0">
                  {(source.usage ?? 'unclear').replace('_', ' ')}
                </Badge>
              </CardHeader>
              <CardContent className="flex flex-col gap-2 text-xs">
                <div className="flex flex-wrap gap-1">
                  {(source.labels ?? []).map((label) => (
                    <Badge key={label} className="px-1.5 py-0 text-[10px]">
                      {label}
                    </Badge>
                  ))}
                </div>
                <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-muted-foreground">
                  <dt>size</dt>
                  <dd className="font-mono">
                    {source.items ? source.items.toLocaleString() : '—'} {source.item_label}
                  </dd>
                  <dt>licence</dt>
                  <dd>{source.license}</dd>
                  <dt>access</dt>
                  <dd>{source.access}</dd>
                  <dt>checked</dt>
                  <dd className="font-mono">{source.verified_on}</dd>
                </dl>
                <p className="leading-relaxed text-muted-foreground">{source.notes}</p>
                {source.paper && (
                  <a
                    href={source.paper}
                    target="_blank"
                    rel="noreferrer noopener"
                    className="inline-flex items-center gap-1 text-primary hover:underline"
                  >
                    paper <ExternalLink className="h-3 w-3" />
                  </a>
                )}
              </CardContent>
            </Card>
          ))}
        </div>
      )}

      <Card className="border-dashed">
        <CardHeader>
          <CardTitle className="text-sm">Looking for something not listed</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-col gap-2 text-xs text-muted-foreground">
          {(sources.data?.directories ?? []).map((directory) => (
            <div key={directory.url}>
              <a
                href={directory.url}
                target="_blank"
                rel="noreferrer noopener"
                className="inline-flex items-center gap-1 font-medium text-foreground hover:underline"
              >
                {directory.name}
                <ExternalLink className="h-3 w-3" />
              </a>
              <p className="leading-relaxed">{directory.notes}</p>
            </div>
          ))}
          <p className="pt-1 leading-relaxed">
            Read the licence in the original repository, never on a mirror. A dataset re-uploaded
            as MIT that is CC BY-NC upstream is common, and the copy is not the one a court reads.
          </p>
        </CardContent>
      </Card>
    </div>
  );
}
