import * as React from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import {
  AlertTriangle,
  Download,
  ExternalLink,
  Heart,
  Play,
  Image as ImageIcon,
  Search,
  ShieldCheck,
  Sparkles,
  Table2,
} from 'lucide-react';

import {
  importImages,
  listHubRows,
  listHubs,
  publishDataset,
  searchHubs,
} from '@/api/generated/sdk.gen';
import type { HubColumn, HubDataset, HubKind, UsageRights } from '@/api/generated/types.gen';
import { FlowResultView } from '@/components/flow-preview';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import { runQuery, simulateQuery, type FlowResult } from '@/lib/flow';
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

  // The pipeline is generated from the selection and then editable, and the
  // two have to stay distinguishable. Every hub lays its files out differently
  // — a search result names a *dataset*, so the generated mapping can only
  // point `uri` at the dataset's page, and turning that into one row per image
  // is a query somebody writes. `edited` holds what they wrote; null means
  // "still following the selection".
  // What the corpus says it holds. The generated script is written from this
  // rather than from a shape assumed here, because there is no such shape: a
  // hub dataset's columns are its author's.
  const columns = useQuery({
    queryKey: ['hub-columns', selected?.hub, selected?.id],
    enabled: selected?.hub === 'huggingface',
    retry: false,
    queryFn: async () => {
      const response = await listHubRows({ query: { dataset: selected?.id ?? '', limit: 1 } });
      if (!response.data) throw disabled(response.error);
      return response.data.columns;
    },
  });

  const generated = React.useMemo(
    () => discoveryPipeline(query, hub, selected, columns.data ?? []),
    [query, hub, selected, columns.data],
  );
  const [edited, setEdited] = React.useState<string | null>(null);
  const [view, setView] = React.useState<'rows' | 'images'>('rows');
  const pipeline = edited ?? generated;
  // An edit made against one dataset, still on screen after picking another.
  // Regenerating silently would discard typed work; following the selection
  // silently would import from the wrong place. So it says so and offers both.
  const drifted = edited !== null && edited !== generated;

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
                The Flow pipeline below produces the rows, and it is editable: a search names a
                dataset rather than its files, so what is generated is a starting point that imports
                one row for the corpus itself. Always dry-run first — a group key mapped from a
                filename gives every image its own family, which turns the split back into a
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
              <div className="mb-1 flex flex-wrap items-center justify-between gap-2">
                <span className="text-xs text-muted-foreground">
                  Flow PHP · {edited === null ? 'generated from the selection' : 'edited by hand'}
                </span>
                {edited === null ? null : (
                  <button
                    type="button"
                    onClick={() => setEdited(null)}
                    className="text-xs text-primary hover:underline"
                  >
                    Regenerate from the selection
                  </button>
                )}
              </div>
              <textarea
                value={pipeline}
                onChange={(event) => setEdited(event.target.value)}
                spellCheck={false}
                rows={14}
                className="id w-full resize-y rounded-md border border-border bg-muted/30 p-3 text-xs outline-none focus-visible:ring-2 focus-visible:ring-primary"
              />
              {drifted ? (
                <p className="mt-1 text-xs text-warning">
                  The search or the selected dataset has moved on since this was edited. What runs
                  is what is written here, not what the selection would generate.
                </p>
              ) : null}
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

      {simulate.data ? (
        <div className="flex flex-wrap items-center gap-2">
          <Button variant={view === 'rows' ? 'default' : 'outline'} onClick={() => setView('rows')}>
            <Table2 className="h-3.5 w-3.5" /> Rows
          </Button>
          <Button
            variant={view === 'images' ? 'default' : 'outline'}
            onClick={() => setView('images')}
          >
            <ImageIcon className="h-3.5 w-3.5" /> Images
          </Button>
        </div>
      ) : null}

      {simulate.data && view === 'images' ? (
        <ImageStrip result={simulate.data} />
      ) : (
        <FlowResultView
          result={simulate.data}
          error={simulate.error}
          emptyTitle="No rows previewed yet"
          previewImages
        />
      )}
    </div>
  );
}

/**
 * The previewed rows as pictures.
 *
 * Not a gallery: `uri` is the column the import reads, so this is a direct
 * check of the mapping. A tile that will not load is a `uri` pointing at
 * something that is not an image, and the commonest reason is the generated
 * pipeline itself — a hub search names a *dataset*, so `uri` starts out on the
 * dataset's page and turning that into one row per image is the edit the
 * textarea above exists for. Saying "not an image" is the finding; a broken
 * image icon would be a rendering accident.
 *
 * The browser loads these from the hub directly. Nothing is proxied through
 * aiwatcher and nothing is stored — an import records the URI, never bytes.
 *
 * A `.map` rather than a virtual list, and eager images, because a simulation
 * is capped by the Flow service — the ceiling this grid needs is one somebody
 * else already enforces.
 */
function ImageStrip({ result }: { result: FlowResult }) {
  // Only failure is tracked. A placeholder sits *behind* every tile and the
  // image paints over it when it arrives, so nothing has to observe a `load`
  // event to show a picture — a state machine gated on one renders a permanent
  // "loading…" wherever that event does not arrive, which is a lie told by the
  // thing meant to prevent one.
  const [failed, setFailed] = React.useState<Record<string, true>>({});

  const tiles = result.rows
    .map((row) => ({
      // `uri` and `group_id` are the *import's* column names, not the
      // corpus's — they are what the query above was written to produce and
      // what `toImportRows` reads. A row that named them something else is a
      // pipeline that has not been pointed at an import yet.
      uri: typeof row.uri === 'string' ? row.uri : '',
      caption: String(row.group_id ?? ''),
    }))
    .filter((tile) => tile.uri !== '');

  if (tiles.length === 0) {
    return (
      <EmptyState
        title="No uri to show"
        hint="The preview renders what `uri` points at, and these rows carry none."
      />
    );
  }

  return (
    <Card className="overflow-hidden">
      <div className="border-b border-border px-3 py-2 text-xs text-muted-foreground">
        {tiles.length} of {result.rows.length} rows carry a uri
      </div>
      <div className="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(9rem,1fr))]">
        {tiles.map((tile, index) => (
          <figure key={`${tile.uri}-${index}`} className="min-w-0">
            <div className="relative h-28 rounded-md border border-dashed border-border bg-muted/20">
              <div className="absolute inset-0 flex items-center justify-center px-2 text-center text-[11px] text-muted-foreground">
                {failed[tile.uri] ? 'not an image' : null}
              </div>
              <img
                src={tile.uri}
                alt=""
                hidden={Boolean(failed[tile.uri])}
                onError={() => setFailed((previous) => ({ ...previous, [tile.uri]: true }))}
                className="relative h-28 w-full object-contain"
              />
            </div>
            <figcaption
              className="mt-1 truncate text-[11px] text-muted-foreground"
              title={tile.caption || tile.uri}
            >
              {tile.caption || tile.uri}
            </figcaption>
            <a
              href={tile.uri}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center gap-1 text-[11px] text-primary hover:underline"
            >
              open <ExternalLink className="h-3 w-3" />
            </a>
          </figure>
        ))}
      </div>
    </Card>
  );
}

/**
 * How many pictures one generated import reads.
 *
 * The API caps it at 100 and this asks for that: a corpus is imported once,
 * and the number nobody has to think about is the one that reaches the cap the
 * route already enforces. Narrowing it is an edit in the script.
 */
const IMPORT_LIMIT = 100;

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
 * A first draft of the pipeline that turns a hub dataset into import rows.
 *
 * Every column name in it comes from the corpus, and every mapping decision is
 * a line somebody can read and change. That is the point rather than a
 * convenience: aiwatcher does not know which column of somebody else's dataset
 * is the picture, whether `indices` is an id or a label, or whether two rows
 * are two buildings or two photographs of one. It guesses here, out loud, in
 * text the reader is already looking at — and a guess in a script is a
 * different thing from a guess in a route, because only one of them can be
 * corrected by the person who knows.
 *
 * The guesses are: the first column the hub typed as an image, or failing that
 * the first it sent as bytes; the first string column as a caption; and one
 * family per row. The last is the one worth checking — see the import route's
 * own warning about it.
 */
function discoveryPipeline(
  query: string,
  hub: HubKind | 'all',
  selected: HubDataset | null,
  columns: HubColumn[],
): string {
  if (selected && selected.hub === 'huggingface' && columns.length > 0) {
    const picture =
      columns.find((column) => column.kind === 'Image') ??
      columns.find((column) => column.dtype === 'binary');
    const caption = columns.find(
      (column) => column.dtype === 'string' && column.name !== picture?.name,
    );
    if (!picture) {
      return [
        'data_frame()',
        `    ->read(hub_rows, dataset: '${phpString(selected.id)}', limit: ${IMPORT_LIMIT})`,
        '    // This corpus declares no image or binary column. Its columns are:',
        `    //   ${columns.map((column) => column.name).join(', ')}`,
        "    // Point 'uri' at whichever one addresses a picture.",
        '    ->write(to_output(truncate: false))',
        '    ->run();',
      ].join('\n');
    }

    const lines = [
      'data_frame()',
      `    ->read(hub_rows, dataset: '${phpString(selected.id)}', limit: ${IMPORT_LIMIT})`,
      `    // '${picture.name}' is this corpus's ${picture.kind === 'Image' ? 'image column' : 'binary column'}, of: ${columns.map((column) => column.name).join(', ')}`,
      picture.kind === 'Image'
        ? `    ->withEntry('uri', array_get(ref('row'), '${picture.name}.src'))`
        : `    ->withEntry('uri', array_get(ref('row'), '${picture.name}'))`,
    ];

    if (picture.kind === 'Image') {
      lines.push(
        `    ->withEntry('width', array_get(ref('row'), '${picture.name}.width'))`,
        `    ->withEntry('height', array_get(ref('row'), '${picture.name}.height'))`,
      );
    } else {
      // A binary column carries no size, and nothing here can measure a
      // picture it has not downloaded. The import reads it out of the bytes it
      // stores.
      lines.push("    ->withEntry('width', lit(0))", "    ->withEntry('height', lit(0))");
    }

    // One family per row. Right for a corpus of unrelated pictures, wrong the
    // moment one publishes a mirror or a second storey of the same building —
    // and then this is the line to change, to whichever column names the
    // subject.
    lines.push(
      `    ->withEntry('group_id', concat(lit('${phpString(selected.id)}/'), ref('row_index')))`,
    );

    const selects = [
      "        ref('uri')",
      "        ref('width')",
      "        ref('height')",
      "        ref('group_id')",
    ];
    if (caption) {
      lines.push(`    ->withEntry('caption', array_get(ref('row'), '${caption.name}'))`);
      selects.push("        ref('caption')");
    }

    lines.push('    ->select(', selects.join(',\n'), '    )');
    lines.push('    ->write(to_output(truncate: false))', '    ->run();');
    return lines.join('\n');
  }

  const lines = ['data_frame()'];
  const args = [`q: '${phpString(query)}'`];
  if (hub !== 'all') args.push(`hub: '${phpString(hub)}'`);
  lines.push(`    ->read(hub_datasets, ${args.join(', ')})`);

  if (selected) {
    lines.push(`    ->filter(ref('id')->same(lit('${phpString(selected.id)}')))`);
  }

  lines.push(
    // The corpus itself rather than its contents, which is all there is for a
    // hub that publishes archives instead of rows. The registry refuses it —
    // `uri` is a web page and the dimensions are zero — and that refusal is
    // the honest state until somebody points this at the files.
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
