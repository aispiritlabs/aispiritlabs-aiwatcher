import * as React from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import {
  AlertTriangle,
  Download,
  ExternalLink,
  Heart,
  Play,
  Search,
  ShieldCheck,
  Sparkles,
} from 'lucide-react';

import { importImages, listHubs, publishDataset, searchHubs } from '@/api/generated/sdk.gen';
import type { HubDataset, HubKind, UsageRights } from '@/api/generated/types.gen';
import { FlowResultView } from '@/components/flow-preview';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import { runQuery, simulateQuery } from '@/lib/flow';
import { cn } from '@/lib/utils';

/**
 * Finding a corpus on Kaggle or Hugging Face, and importing one.
 *
 * The screen has one job beyond the obvious, and it is the reason the licence
 * column is laid out the way it is: **a mirror's licence field is not a
 * licence**. It is what somebody typed when uploading a copy, and a CC BY-NC
 * corpus re-uploaded as MIT is common enough that treating it as authoritative
 * would be worse than showing nothing.
 *
 * So the row shows the hub's claim and aiwatcher's verdict as two separate
 * things, never merged. A row that matched the curated table carries a
 * `verified` badge naming the corpus somebody actually read the licence for;
 * every other row says `unclear`, whatever the mirror claims. The rights
 * selector defaults to Unknown for exactly that reason, and the panel never
 * pre-fills it from a hub's word.
 *
 * The import itself is a Flow PHP pipeline, not a fetch loop here. Every hub
 * lays its files out differently, and the mapping belongs somewhere versioned
 * and readable rather than in a `switch` in a React component.
 */

const RIGHTS_OPTIONS: { value: string; label: string; hint: string }[] = [
  {
    value: 'unknown',
    label: 'Unknown — nobody has checked',
    hint: 'The safe default. A commercial export excludes these and says so in its manifest.',
  },
  {
    value: 'research_only',
    label: 'Research only',
    hint: 'CC BY-NC and everything shaped like it. Usable for research exports, never a commercial one.',
  },
  {
    value: 'licensed',
    label: 'Licensed for commercial use',
    hint: 'Only after reading the licence at the original — not on the mirror’s word.',
  },
  {
    value: 'owned',
    label: 'Owned, or granted to us',
    hint: 'Produced here, or supplied with a grant covering training and the resulting weights.',
  },
];

export function HubDiscovery({
  project,
  onProjectChange,
  windowSeconds,
}: {
  project: string;
  onProjectChange: (project: string) => void;
  windowSeconds: number;
}) {
  const [draft, setDraft] = React.useState('');
  const [query, setQuery] = React.useState('');
  const [hub, setHub] = React.useState<HubKind | 'all'>('all');
  const [selected, setSelected] = React.useState<HubDataset | null>(null);
  const [rightsKind, setRightsKind] = React.useState('unknown');
  const [license, setLicense] = React.useState('');
  const [datasetName, setDatasetName] = React.useState('discovery/hub-search');

  const hubs = useQuery({
    queryKey: ['dataset-hubs'],
    queryFn: async () => {
      const response = await listHubs();
      if (!response.data) throw disabled(response.error);
      return response.data;
    },
    retry: false,
  });

  const results = useQuery({
    enabled: hubs.isSuccess,
    queryKey: ['dataset-hubs', 'search', query, hub],
    queryFn: async () => {
      const response = await searchHubs({
        query: { q: query || undefined, hub: hub === 'all' ? undefined : hub },
      });
      if (!response.data) throw disabled(response.error);
      return response.data;
    },
  });

  // A 250 ms debounce, the same as every other search box in the panel. This
  // one reaches a third party, so it is the one place the delay is also
  // somebody else's rate limit.
  React.useEffect(() => {
    const timer = setTimeout(() => setQuery(draft), 250);
    return () => clearTimeout(timer);
  }, [draft]);

  const pipeline = React.useMemo(
    () => discoveryPipeline(query, hub, selected),
    [query, hub, selected],
  );

  const simulate = useMutation({
    mutationFn: () => simulateQuery(pipeline, windowSeconds || undefined),
  });

  const dryRun = useMutation({
    mutationFn: async () => runImport({ preview: true }),
  });
  const commit = useMutation({
    mutationFn: async () => runImport({ preview: false }),
  });

  async function runImport({ preview }: { preview: boolean }) {
    if (!selected) throw new Error('Choose a dataset first.');
    if (!project.trim()) throw new Error('Name the annotation project to import into.');

    // The Flow pipeline produces the rows. The panel never invents them: the
    // whole point of routing this through a query is that the mapping is
    // versioned and readable rather than hidden in a component.
    const result = await runQuery(pipeline, windowSeconds || undefined);

    const report = await importImages({
      body: {
        project,
        dry_run: preview,
        rights: buildRights(rightsKind, license, selected),
        source: {
          hub: selected.hub,
          dataset_id: selected.id,
          url: selected.url,
          claimed_license: selected.claimed_license ?? '',
          curated_source: selected.curated_source ?? undefined,
          curated_usage: selected.curated_source ? selected.usage : undefined,
          pipeline,
        },
        rows: toImportRows(result.rows),
      },
    });
    if (!report.data) throw disabled(report.error);

    // Provenance the import itself cannot carry: a hub search is a moment in
    // time, and the rows it returned are what somebody decided from. Recorded
    // as an immutable version so "why did we import that" has an answer after
    // the hub's listing has changed.
    if (!preview) {
      await publishDataset({
        body: {
          name: datasetName,
          description: `Hub search that produced the import into ${project}.`,
          pipeline,
          columns: result.columns,
          items: result.rows,
          source: result.source,
        },
      });
    }
    return { result, report: report.data };
  }

  if (hubs.isError) {
    return (
      <Card className="p-4">
        <EmptyState title="No dataset hub is configured" hint={hubs.error.message} />
        <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
          This is a 501 rather than an empty list on purpose. An empty search result would read as
          “there is no such corpus”, which is a claim about the world rather than about this
          deployment.
        </p>
      </Card>
    );
  }

  const report = commit.data?.report ?? dryRun.data?.report;

  return (
    <div className="flex min-w-0 flex-col gap-4">
      <Card className="overflow-hidden">
        <div className="flex items-start gap-3 border-b border-border p-4">
          <div className="rounded-md bg-primary/10 p-2 text-primary">
            <Search className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <h2 className="text-sm font-semibold">Search public dataset hubs</h2>
            <p className="mt-0.5 text-xs text-muted-foreground">{hubs.data?.notice}</p>
          </div>
        </div>

        <div className="flex flex-col gap-3 p-4">
          <div className="flex flex-wrap items-center gap-2">
            <input
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              placeholder="what this corpus is of…"
              className="h-9 min-w-64 flex-1 rounded-md border border-border bg-card px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
            />
            {(['all', 'huggingface', 'kaggle'] as const).map((value) => {
              const status = hubs.data?.hubs.find((entry) => entry.hub === value);
              const off = value !== 'all' && status?.configured === false;
              return (
                <Button
                  key={value}
                  size="sm"
                  variant={hub === value ? 'default' : 'outline'}
                  disabled={off}
                  title={off ? `Not configured. Set ${status?.variable}.` : undefined}
                  onClick={() => setHub(value)}
                >
                  {value === 'all' ? 'Both hubs' : value === 'kaggle' ? 'Kaggle' : 'Hugging Face'}
                </Button>
              );
            })}
          </div>

          {results.data?.hubs
            .filter((status) => status.error || !status.configured)
            .map((status) => (
              <p key={status.hub} className="text-xs text-warning">
                {status.configured
                  ? `${status.hub} did not answer: ${status.error}`
                  : `${status.hub} is not configured — set ${status.variable}.`}
              </p>
            ))}

          {results.isPending ? (
            <p className="text-sm text-muted-foreground">Searching…</p>
          ) : results.data?.results.length ? (
            <div className="max-h-96 divide-y divide-border/50 overflow-y-auto rounded-md border border-border">
              {results.data.results.map((row) => (
                <HubRow
                  key={`${row.hub}:${row.id}`}
                  row={row}
                  selected={selected?.id === row.id && selected.hub === row.hub}
                  onSelect={() => {
                    setSelected(row);
                    // Never pre-filled from the mirror. Somebody has to say so.
                    setRightsKind('unknown');
                    setLicense(row.claimed_license ?? '');
                  }}
                />
              ))}
            </div>
          ) : (
            <EmptyState title="Nothing matched" hint="Try a broader term, or the other hub." />
          )}
        </div>
      </Card>

      {selected ? (
        <Card className="overflow-hidden">
          <div className="flex items-start gap-3 border-b border-border p-4">
            <div className="rounded-md bg-primary/10 p-2 text-primary">
              <Download className="h-4 w-4" />
            </div>
            <div>
              <h2 className="text-sm font-semibold">Import into an annotation project</h2>
              <p className="mt-0.5 text-xs text-muted-foreground">
                The Flow pipeline below produces the rows. Always dry-run first — a group key mapped
                from a filename gives every image its own family, which turns the split back into a
                per-image one with nothing in the numbers to say so.
              </p>
            </div>
          </div>

          <div className="flex flex-col gap-4 p-4">
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="flex flex-col gap-1 text-xs">
                <span className="text-muted-foreground">Annotation project</span>
                <input
                  value={project}
                  onChange={(event) => onProjectChange(event.target.value)}
                  placeholder="corpora/first"
                  className={CONTROL}
                />
              </label>
              <label className="flex flex-col gap-1 text-xs">
                <span className="text-muted-foreground">Record the search as</span>
                <input
                  value={datasetName}
                  onChange={(event) => setDatasetName(event.target.value)}
                  className={CONTROL}
                />
              </label>
            </div>

            <div className="flex flex-col gap-1 text-xs">
              <span className="text-muted-foreground">What may be done with these images</span>
              <select
                value={rightsKind}
                onChange={(event) => setRightsKind(event.target.value)}
                className={CONTROL}
              >
                {RIGHTS_OPTIONS.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
              <p className="text-[11px] text-muted-foreground">
                {RIGHTS_OPTIONS.find((option) => option.value === rightsKind)?.hint}
              </p>
              {rightsKind !== 'unknown' && rightsKind !== 'owned' ? (
                <input
                  value={license}
                  onChange={(event) => setLicense(event.target.value)}
                  placeholder="the licence, as it reads at the original"
                  className={cn(CONTROL, 'mt-1')}
                />
              ) : null}
              {selected.curated_source && selected.usage === 'non_commercial' ? (
                <p className="mt-1 flex items-start gap-1.5 text-[11px] text-danger">
                  <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
                  Somebody read {selected.curated_source}’s licence at the original and recorded it
                  as research-only. Importing it as commercially usable is refused.
                </p>
              ) : null}
            </div>

            <div>
              <span className="mb-1 block text-xs text-muted-foreground">Generated Flow PHP</span>
              <pre className="id max-h-60 overflow-auto rounded-md border border-border bg-muted/30 p-3 text-xs">
                {pipeline}
              </pre>
            </div>

            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                onClick={() => simulate.mutate()}
                disabled={simulate.isPending}
              >
                {simulate.isPending ? <Spinner /> : <Sparkles className="h-3.5 w-3.5" />} Preview
                rows
              </Button>
              <Button
                variant="outline"
                onClick={() => dryRun.mutate()}
                disabled={dryRun.isPending || commit.isPending || !project.trim()}
              >
                {dryRun.isPending ? <Spinner /> : <ShieldCheck className="h-3.5 w-3.5" />} Dry run
              </Button>
              <Button
                onClick={() => commit.mutate()}
                disabled={commit.isPending || dryRun.isPending || !project.trim()}
              >
                {commit.isPending ? <Spinner /> : <Play className="h-3.5 w-3.5" />} Import
              </Button>
            </div>

            {[dryRun.error, commit.error].filter(Boolean).map((error, index) => (
              <p key={index} className="text-sm text-danger">
                {(error as Error).message}
              </p>
            ))}

            {report ? <ImportReportView report={report} /> : null}
          </div>
        </Card>
      ) : null}

      <FlowResultView
        result={simulate.data}
        error={simulate.error}
        emptyTitle="No rows previewed yet"
      />
    </div>
  );
}

const CONTROL =
  'h-9 rounded-md border border-border bg-card px-3 text-card-foreground text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary';

function HubRow({
  row,
  selected,
  onSelect,
}: {
  row: HubDataset;
  selected: boolean;
  onSelect: () => void;
}) {
  const verified = Boolean(row.curated_source);
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        'w-full p-3 text-left transition-colors hover:bg-accent/30',
        selected && 'bg-accent/50',
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <p className="truncate text-sm font-medium">{row.title || row.id}</p>
          <p className="truncate text-xs text-muted-foreground">{row.id}</p>
        </div>
        <Badge tone="neutral">{row.hub}</Badge>
      </div>
      {row.summary ? (
        <p className="mt-1 line-clamp-2 text-xs text-muted-foreground">{row.summary}</p>
      ) : null}
      <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11px]">
        {/* Two separate facts, never merged into one badge. */}
        <Badge tone={verified ? (row.usage === 'commercial' ? 'success' : 'danger') : 'warning'}>
          {verified ? `verified: ${row.usage}` : 'unclear'}
        </Badge>
        {row.claimed_license ? (
          <span className="text-muted-foreground">
            mirror claims <code className="id">{row.claimed_license}</code>
          </span>
        ) : (
          <span className="text-muted-foreground">the mirror states no licence</span>
        )}
        {verified ? (
          <span className="text-muted-foreground">· read at {row.curated_source}</span>
        ) : null}
        {typeof row.downloads === 'number' ? (
          <span className="ml-auto flex items-center gap-1 text-muted-foreground">
            <Download className="h-3 w-3" />
            {row.downloads.toLocaleString()}
          </span>
        ) : null}
        {typeof row.likes === 'number' ? (
          <span className="flex items-center gap-1 text-muted-foreground">
            <Heart className="h-3 w-3" />
            {row.likes.toLocaleString()}
          </span>
        ) : null}
        <a
          href={row.url}
          target="_blank"
          rel="noreferrer noopener"
          onClick={(event) => event.stopPropagation()}
          className="flex items-center gap-1 text-primary hover:underline"
        >
          open <ExternalLink className="h-3 w-3" />
        </a>
      </div>
    </button>
  );
}

function ImportReportView({
  report,
}: {
  report: {
    accepted: number;
    rejected: number;
    families: number;
    dry_run: boolean;
    warnings?: string[];
    outcomes: unknown[];
  };
}) {
  return (
    <Card className="p-3 text-sm">
      <p>
        {report.dry_run ? 'Dry run: ' : 'Imported '}
        <strong>{report.accepted}</strong> image{report.accepted === 1 ? '' : 's'} across{' '}
        <strong>{report.families}</strong> famil{report.families === 1 ? 'y' : 'ies'}
        {report.rejected ? `, ${report.rejected} refused` : ''}.
      </p>
      {report.warnings?.length ? (
        <ul className="mt-2 flex flex-col gap-1.5">
          {report.warnings.map((warning) => (
            <li key={warning} className="flex items-start gap-1.5 text-xs text-warning">
              <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0" />
              {warning}
            </li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

/**
 * The pipeline that turns a hub search into rows the import route accepts.
 *
 * Generated rather than hand-written so the common case is one click, and
 * shown rather than hidden so the uncommon case is an edit. It is deliberately
 * incomplete for a real corpus: a hub's *file listing* is what carries image
 * dimensions and the building a plan belongs to, and no two hubs expose it the
 * same way. What this produces is the shape, with the three columns somebody
 * has to map themselves marked as such.
 */
function discoveryPipeline(
  query: string,
  hub: HubKind | 'all',
  selected: HubDataset | null,
): string {
  const lines = ['data_frame()'];
  const args = [`search: '${phpString(query)}'`];
  if (hub !== 'all') args.push(`hub: '${phpString(hub)}'`);
  lines.push(`    ->read(hub_datasets, ${args.join(', ')})`);

  if (selected) {
    lines.push(`    ->filter(ref('id')->same(lit('${phpString(selected.id)}')))`);
  }

  lines.push(
    // The three columns a hub search cannot answer, spelled out rather than
    // guessed. `group_id` in particular: derived from `id` it would give every
    // image its own family, which is the mistake the import route warns about.
    "    ->withEntry('uri', ref('url'))",
    "    ->withEntry('group_id', ref('id'))",
    "    ->withEntry('width', lit(0))",
    "    ->withEntry('height', lit(0))",
    '    ->select(',
    "        ref('uri'),",
    "        ref('group_id'),",
    "        ref('width'),",
    "        ref('height'),",
    "        ref('claimed_license'),",
    "        ref('usage')",
    '    )',
    '    ->write(to_output(truncate: false))',
    '    ->run();',
  );
  return lines.join('\n');
}

function toImportRows(rows: Record<string, unknown>[]) {
  return rows.map((row) => ({
    image_id: typeof row.image_id === 'string' ? row.image_id : undefined,
    uri: String(row.uri ?? ''),
    width: Number(row.width ?? 0),
    height: Number(row.height ?? 0),
    group_id: String(row.group_id ?? ''),
    level: typeof row.level === 'string' ? row.level : undefined,
  }));
}

function buildRights(kind: string, license: string, row: HubDataset): UsageRights {
  switch (kind) {
    case 'owned':
      return { kind: 'owned', grant: license || 'granted for training and the resulting weights' };
    case 'licensed':
      return {
        kind: 'licensed',
        license: license || row.claimed_license || 'unstated',
        url: row.url,
      };
    case 'research_only':
      return {
        kind: 'research_only',
        license: license || row.claimed_license || 'unstated',
        url: row.url,
      };
    default:
      return { kind: 'unknown' };
  }
}

function phpString(value: string): string {
  return value.replaceAll('\\', '\\\\').replaceAll("'", "\\'");
}

function disabled(error: unknown): Error {
  if (
    error &&
    typeof error === 'object' &&
    'message' in error &&
    typeof error.message === 'string'
  ) {
    return new Error(error.message);
  }
  return new Error('The request was refused.');
}
