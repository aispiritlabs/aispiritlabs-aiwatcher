import * as React from 'react';
import { Link, createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ArrowLeft, GitCompare, Pencil, RefreshCw } from 'lucide-react';
import { z } from 'zod';

import {
  getOptimization,
  getPrompt,
  getPromptVersion,
  publishPrompt,
  rebuildPrompt,
  setPromptLabel,
} from '@/api/generated/sdk.gen';
import type { OptimizationSummary, PromptHead, PromptVersion } from '@/api/generated/types.gen';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  IdChip,
  Spinner,
  Stat,
} from '@/components/ui/primitives';
import {
  Delta,
  EvaluationLink,
  OriginBadge,
  OutcomeBadge,
  OverfitGap,
  REJECTION_TEXT,
  RunCost,
  SplitDeltas,
  Variables,
  VersionArrow,
} from '@/components/prompt-bits';
import { PromptDiff } from '@/components/prompt-diff';
import { RegistryDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import { ErrorText } from '@/routes/prompts.index';
import { cn, formatTime } from '@/lib/utils';

/**
 * One prompt: every version of it, and every optimisation run against it.
 *
 * ```text
 * head            labels, description, and the index of what is stored
 * ├── versions    immutable, content-addressed — the id is sha256(text)
 * └── optimisations   baseline → candidate, dev scores, held-out scores, a verdict
 * ```
 *
 * ## The verdict is not the optimiser's
 *
 * `outcome` on every row here was computed by the server from the held-out
 * scores and from what the candidate did to the baseline's variables. An
 * optimiser selected its candidate by maximising the number it then reports,
 * which makes it the last thing that should grade it — so the panel shows the
 * dev gain and the held-out gain side by side rather than a single score, and
 * flags the gap between them.
 *
 * ## Why the diff is a first-class view
 *
 * A rewritten prompt with a better score and no visible change behind it is the
 * thing people mean when they say they do not trust an optimiser. The diff is
 * how "it scored 0.07 higher" becomes "it added a sentence telling the model to
 * read the dimension lines" — see `lib/diff.ts`.
 */

const searchSchema = z.object({
  /** The version shown in the pane. Defaults to whatever is current. */
  version: z.string().optional(),
  /** `text` or a diff against `against`. */
  view: z.enum(['text', 'diff']).optional(),
  /** The version the diff compares against. Defaults to the parent. */
  against: z.string().optional(),
  /** The optimisation whose report is expanded. */
  optimization: z.string().optional(),
});

export const Route = createFileRoute('/prompts/$name')({
  validateSearch: searchSchema,
  component: PromptPage,
});

function PromptPage() {
  const { name } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const [editing, setEditing] = React.useState(false);

  const select = React.useCallback(
    (next: Partial<z.infer<typeof searchSchema>>) => {
      void navigate({ search: (previous) => ({ ...previous, ...next }) });
    },
    [navigate],
  );

  const invalidate = React.useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: ['prompt', name] });
    await queryClient.invalidateQueries({ queryKey: ['prompts'] });
  }, [name, queryClient]);

  const prompt = useQuery({
    queryKey: ['prompt', name],
    queryFn: async () => {
      const response = await getPrompt({ path: { name } });
      if (!response.data) throw response.error ?? new Error('failed to load the prompt');
      return response.data;
    },
    retry: (count, error) => !isRegistryDisabled(error) && count < 2,
  });

  const head = prompt.data?.head;
  const selectedId = search.version ?? head?.labels?.production ?? head?.versions?.[0]?.version_id;

  // The current version arrives with the detail; anything else is one more
  // request. Versions are immutable, so once fetched they never go stale —
  // hence the infinite `staleTime`.
  const selected = useQuery({
    queryKey: ['prompt-version', name, selectedId],
    enabled: Boolean(selectedId),
    staleTime: Infinity,
    queryFn: async () => {
      const current = prompt.data?.current;
      if (current && current.version_id === selectedId) return current;
      const response = await getPromptVersion({
        path: { name, version_id: selectedId as string },
      });
      if (!response.data) throw response.error ?? new Error('failed to load the version');
      return response.data;
    },
  });

  const againstId = search.against ?? selected.data?.parent ?? undefined;
  const against = useQuery({
    queryKey: ['prompt-version', name, againstId],
    enabled: search.view === 'diff' && Boolean(againstId),
    staleTime: Infinity,
    queryFn: async () => {
      const response = await getPromptVersion({
        path: { name, version_id: againstId as string },
      });
      if (!response.data) throw response.error ?? new Error('failed to load the version');
      return response.data;
    },
  });

  const promote = useMutation({
    mutationFn: async (versionId: string) => {
      const response = await setPromptLabel({
        path: { name, label: 'production' },
        body: { version_id: versionId },
      });
      if (!response.data) throw response.error ?? new Error('failed to move the label');
      return response.data;
    },
    onSuccess: invalidate,
  });

  const rebuild = useMutation({
    mutationFn: async () => {
      const response = await rebuildPrompt({ path: { name } });
      if (!response.data) throw response.error ?? new Error('failed to rebuild');
      return response.data;
    },
    onSuccess: invalidate,
  });

  const publish = useMutation({
    mutationFn: async (input: { text: string; notes?: string; label?: string }) => {
      const response = await publishPrompt({
        body: { name, parent: selectedId, ...input },
      });
      if (!response.data) throw response.error ?? new Error('failed to publish');
      return response.data;
    },
    onSuccess: async (published) => {
      setEditing(false);
      await invalidate();
      select({ version: published.version.version_id, view: 'diff' });
    },
  });

  if (isRegistryDisabled(prompt.error)) return <RegistryDisabled />;
  if (prompt.isLoading) {
    return (
      <div className="flex items-center gap-2 p-10 text-sm text-muted-foreground">
        <Spinner /> loading {name}…
      </div>
    );
  }
  if (!head) {
    return (
      <EmptyState
        title={`No prompt called ${name}`}
        hint="It may have been published under a different name — the list has the ones that exist."
      />
    );
  }

  const production = head.labels?.production;
  const versions = head.versions ?? [];
  const optimizations = head.optimizations ?? [];
  const admitted = optimizations.filter((record) => record.outcome === 'admitted').length;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <Link
            to="/prompts"
            className="mb-1 inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
          >
            <ArrowLeft className="h-3 w-3" /> All prompts
          </Link>
          <h1 className="text-lg font-semibold">{head.name}</h1>
          {head.description ? (
            <p className="max-w-3xl text-sm text-muted-foreground">{head.description}</p>
          ) : null}
          {head.tags && head.tags.length > 0 ? (
            <div className="mt-1.5 flex flex-wrap gap-1">
              {head.tags.map((tag) => (
                <Badge key={tag}>{tag}</Badge>
              ))}
            </div>
          ) : null}
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" onClick={() => setEditing((open) => !open)} className="gap-1.5">
            <Pencil className="h-3.5 w-3.5" />
            New version
          </Button>
          <Button
            variant="ghost"
            title="Re-derive the index from the objects that are stored. Safe to run at any time; labels survive it."
            disabled={rebuild.isPending}
            onClick={() => rebuild.mutate()}
            className="gap-1.5"
          >
            {rebuild.isPending ? <Spinner /> : <RefreshCw className="h-3.5 w-3.5" />}
            Re-index
          </Button>
        </div>
      </div>

      <Card className="flex flex-wrap gap-8 p-4">
        <Stat
          label="Production"
          value={
            production ? (
              <IdChip value={production.slice(0, 12)} full={production} label="production" />
            ) : (
              <span className="text-sm font-normal text-muted-foreground">unpinned</span>
            )
          }
          hint={production ? undefined : 'a reader gets the newest version'}
        />
        <Stat label="Versions" value={versions.length} />
        <Stat
          label="Optimisations"
          value={`${admitted}/${optimizations.length}`}
          hint="admitted on the held-out split"
        />
        <Stat label="Updated" value={formatTime(head.updated_at)} />
      </Card>

      {editing ? (
        <NewVersionForm
          base={selected.data?.text ?? ''}
          pending={publish.isPending}
          error={publish.error}
          onCancel={() => setEditing(false)}
          onSubmit={(input) => publish.mutate(input)}
        />
      ) : null}

      <div className="grid gap-4 lg:grid-cols-[20rem_1fr]">
        <VersionList
          head={head}
          selectedId={selectedId}
          onSelect={(versionId) => select({ version: versionId })}
        />

        <div className="flex flex-col gap-4">
          <VersionPane
            version={selected.data ?? undefined}
            loading={selected.isLoading}
            isProduction={selected.data?.version_id === production}
            view={search.view ?? 'text'}
            against={against.data}
            againstId={againstId}
            onView={(view) => select({ view })}
            onPromote={() =>
              selected.data ? promote.mutate(selected.data.version_id) : undefined
            }
            promoting={promote.isPending}
            promoteError={promote.error}
          />

          <Optimizations
            name={name}
            records={optimizations}
            expanded={search.optimization}
            onExpand={(optimizationId) =>
              select({
                optimization: optimizationId === search.optimization ? undefined : optimizationId,
              })
            }
            onCompare={(record) =>
              select({
                version: record.candidate,
                against: record.baseline,
                view: 'diff',
              })
            }
          />
        </div>
      </div>
    </div>
  );
}

function VersionList({
  head,
  selectedId,
  onSelect,
}: {
  head: PromptHead;
  selectedId: string | undefined;
  onSelect: (versionId: string) => void;
}) {
  const production = head.labels?.production;
  const versions = head.versions ?? [];
  return (
    <Card className="h-fit overflow-hidden">
      <CardHeader className="border-b border-border">
        <CardTitle>Versions</CardTitle>
        <p className="text-xs text-muted-foreground">Newest first. The id is the text&rsquo;s hash.</p>
      </CardHeader>
      <ul className="max-h-[32rem] overflow-y-auto">
        {versions.map((version) => (
          <li key={version.version_id}>
            <button
              type="button"
              onClick={() => onSelect(version.version_id)}
              className={cn(
                'flex w-full flex-col gap-1 border-b border-border/60 px-4 py-3 text-left transition-colors last:border-0 hover:bg-accent/50',
                version.version_id === selectedId && 'bg-accent',
              )}
            >
              <span className="flex items-center justify-between gap-2">
                <code className="text-xs">{version.version_id.slice(0, 12)}</code>
                {version.version_id === production ? (
                  <Badge tone="success">production</Badge>
                ) : null}
              </span>
              <span className="flex items-center gap-2 text-xs text-muted-foreground">
                {formatTime(version.created_at)}
                {version.author ? <>· {version.author}</> : null}
              </span>
              <span className="flex items-center gap-2">
                <OriginBadge version={version} />
              </span>
              {version.notes ? (
                <span className="line-clamp-2 text-xs text-muted-foreground">{version.notes}</span>
              ) : null}
            </button>
          </li>
        ))}
      </ul>
    </Card>
  );
}

function VersionPane({
  version,
  loading,
  isProduction,
  view,
  against,
  againstId,
  onView,
  onPromote,
  promoting,
  promoteError,
}: {
  version: PromptVersion | undefined;
  loading: boolean;
  isProduction: boolean;
  view: 'text' | 'diff';
  against: PromptVersion | undefined;
  againstId: string | undefined;
  onView: (view: 'text' | 'diff') => void;
  onPromote: () => void;
  promoting: boolean;
  promoteError: unknown;
}) {
  if (loading) {
    return (
      <Card className="flex items-center gap-2 p-10 text-sm text-muted-foreground">
        <Spinner /> loading the version…
      </Card>
    );
  }
  if (!version) {
    return (
      <Card className="p-10">
        <EmptyState title="No version selected" hint="Pick one from the list." />
      </Card>
    );
  }

  return (
    <Card className="overflow-hidden">
      <CardHeader className="flex-row flex-wrap items-center justify-between gap-3 border-b border-border">
        <div className="flex flex-col gap-1">
          <CardTitle className="flex items-center gap-2">
            <IdChip value={version.version_id.slice(0, 12)} full={version.version_id} />
            {isProduction ? <Badge tone="success">production</Badge> : null}
            {version.model ? (
              <span className="text-xs font-normal text-muted-foreground">
                written for {version.model}
              </span>
            ) : null}
          </CardTitle>
          <Variables names={version.variables ?? []} />
        </div>
        <div className="flex items-center gap-2">
          <div className="flex overflow-hidden rounded-md border border-border text-xs">
            <button
              type="button"
              onClick={() => onView('text')}
              className={cn('px-3 py-1.5', view === 'text' && 'bg-accent')}
            >
              Text
            </button>
            <button
              type="button"
              onClick={() => onView('diff')}
              disabled={!againstId}
              title={
                againstId
                  ? 'Compare against the version this one was derived from'
                  : 'This version has no parent to compare against'
              }
              className={cn(
                'flex items-center gap-1 px-3 py-1.5 disabled:opacity-40',
                view === 'diff' && 'bg-accent',
              )}
            >
              <GitCompare className="h-3 w-3" />
              Diff
            </button>
          </div>
          {!isProduction ? (
            <Button
              variant="outline"
              size="sm"
              disabled={promoting}
              onClick={onPromote}
              title="Point the production label at this version. Nothing is deployed by recording evidence — this is the act that deploys."
            >
              {promoting ? <Spinner /> : null}
              Make production
            </Button>
          ) : null}
        </div>
      </CardHeader>

      {promoteError ? (
        <div className="px-4 pt-3">
          <ErrorText error={promoteError} />
        </div>
      ) : null}

      {version.notes ? (
        <p className="border-b border-border/60 px-4 py-2 text-xs text-muted-foreground">
          {version.notes}
        </p>
      ) : null}

      {view === 'diff' ? (
        against ? (
          <PromptDiff before={against.text} after={version.text} />
        ) : (
          <p className="p-4 text-xs text-muted-foreground">Loading the version to compare…</p>
        )
      ) : (
        <pre className="overflow-x-auto whitespace-pre-wrap p-4 text-xs leading-relaxed">
          {version.text}
        </pre>
      )}
    </Card>
  );
}

function Optimizations({
  name,
  records,
  expanded,
  onExpand,
  onCompare,
}: {
  name: string;
  records: OptimizationSummary[];
  expanded: string | undefined;
  onExpand: (optimizationId: string) => void;
  onCompare: (record: OptimizationSummary) => void;
}) {
  return (
    <Card className="overflow-hidden">
      <CardHeader className="border-b border-border">
        <CardTitle>Optimisations</CardTitle>
        <p className="max-w-3xl text-xs text-muted-foreground">
          Newest first. The verdict is the server&rsquo;s, computed from the held-out split — an
          optimiser picked its candidate by maximising the dev number it is reporting.
        </p>
      </CardHeader>
      {records.length === 0 ? (
        <CardContent className="pt-4 text-xs text-muted-foreground">
          Nothing has been run against this prompt yet. An optimiser records one with{' '}
          <code>POST /api/v1/prompts/{name}/optimizations</code>, or with{' '}
          <code>record_deepeval_optimization(...)</code> from the Python SDK.
        </CardContent>
      ) : (
        <ul>
          {records.map((record) => (
            <li key={record.optimization_id} className="border-b border-border/60 last:border-0">
              <div className="flex flex-wrap items-center gap-3 px-4 py-3">
                <OutcomeBadge record={record} />
                <span className="text-sm font-medium">{record.algorithm}</span>
                <SplitDeltas record={record} />
                <OverfitGap gap={record.overfit_gap} />
                <span className="ml-auto flex items-center gap-3">
                  <RunCost record={record} />
                  <EvaluationLink evaluationId={record.evaluation_id} />
                  <Button variant="ghost" size="sm" onClick={() => onCompare(record)}>
                    Diff
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => onExpand(record.optimization_id)}
                  >
                    {expanded === record.optimization_id ? 'Hide' : 'Details'}
                  </Button>
                </span>
              </div>

              <div className="flex flex-wrap items-center gap-3 px-4 pb-3 text-xs text-muted-foreground">
                <VersionArrow from={record.baseline} to={record.candidate} />
                {record.dataset ? <span>on {record.dataset}</span> : null}
                <span>{formatTime(record.started_at)}</span>
              </div>

              {record.outcome === 'rejected' && record.reason ? (
                <p
                  className={cn(
                    'px-4 pb-3 text-xs',
                    record.reason === 'variables_lost' ? 'text-danger' : 'text-muted-foreground',
                  )}
                >
                  Not promoted: {REJECTION_TEXT[record.reason]}
                  {record.variables_lost && record.variables_lost.length > 0
                    ? ` (${record.variables_lost.join(', ')})`
                    : ''}
                  .
                </p>
              ) : null}

              {expanded === record.optimization_id ? (
                <OptimizationDetail name={name} optimizationId={record.optimization_id} />
              ) : null}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

/**
 * Everything one optimisation measured, plus whatever document it attached.
 *
 * Fetched on expand rather than with the list: the report is a producer-supplied
 * blob with no size this panel gets to assume, and a list of fifty rows should
 * not carry fifty of them.
 */
function OptimizationDetail({
  name,
  optimizationId,
}: {
  name: string;
  optimizationId: string;
}) {
  const record = useQuery({
    queryKey: ['optimization', name, optimizationId],
    staleTime: Infinity,
    queryFn: async () => {
      const response = await getOptimization({
        path: { name, optimization_id: optimizationId },
      });
      if (!response.data) throw response.error ?? new Error('failed to load the optimisation');
      return response.data;
    },
  });

  if (record.isLoading) {
    return (
      <p className="flex items-center gap-2 px-4 pb-3 text-xs text-muted-foreground">
        <Spinner /> loading…
      </p>
    );
  }
  if (!record.data) return null;

  // Both splits are optional on the wire: an optimiser that measured only one
  // of them still gets a row, with the other side empty rather than absent.
  const dev = record.data.dev ?? [];
  const test = record.data.test ?? [];
  const metrics = new Set([...dev.map((score) => score.metric), ...test.map((score) => score.metric)]);

  return (
    <div className="border-t border-border/60 bg-muted/20 px-4 py-3">
      <table className="w-full text-xs">
        <thead className="text-muted-foreground">
          <tr>
            <th className="py-1 text-left font-medium">Metric</th>
            <th className="py-1 text-right font-medium">Dev baseline</th>
            <th className="py-1 text-right font-medium">Dev candidate</th>
            <th className="py-1 text-right font-medium">Dev Δ</th>
            <th className="py-1 text-right font-medium">Held out baseline</th>
            <th className="py-1 text-right font-medium">Held out candidate</th>
            <th className="py-1 text-right font-medium">Held out Δ</th>
          </tr>
        </thead>
        <tbody className="tabular-nums">
          {[...metrics].map((metric) => {
            const devScore = dev.find((score) => score.metric === metric);
            const testScore = test.find((score) => score.metric === metric);
            const delta = (score: typeof devScore) =>
              score?.candidate !== undefined &&
              score?.candidate !== null &&
              score?.baseline !== undefined &&
              score?.baseline !== null
                ? score.candidate - score.baseline
                : null;
            return (
              <tr key={metric} className="border-t border-border/40">
                <td className="py-1">
                  {metric}
                  {metric === record.data.primary_metric ? (
                    <span
                      className="ml-2 text-[11px] text-muted-foreground"
                      title="The verdict is decided on this one. A gate with several thresholds is a gate somebody tunes until it opens."
                    >
                      decides
                    </span>
                  ) : null}
                </td>
                <td className="py-1 text-right">{format(devScore?.baseline)}</td>
                <td className="py-1 text-right">{format(devScore?.candidate)}</td>
                <td className="py-1 text-right">
                  <Delta value={delta(devScore)} />
                </td>
                <td className="py-1 text-right">{format(testScore?.baseline)}</td>
                <td className="py-1 text-right">{format(testScore?.candidate)}</td>
                <td className="py-1 text-right">
                  <Delta value={delta(testScore)} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>

      {record.data.report ? (
        <details className="mt-3">
          <summary className="cursor-pointer text-xs text-muted-foreground">
            Report from the optimiser
          </summary>
          <pre className="mt-2 max-h-80 overflow-auto rounded border border-border bg-background p-3 text-[11px]">
            {JSON.stringify(record.data.report, null, 2)}
          </pre>
        </details>
      ) : null}
    </div>
  );
}

function format(value: number | null | undefined): string {
  return value === null || value === undefined ? '—' : value.toFixed(3);
}

/**
 * Editing a prompt in the panel.
 *
 * Pre-filled with the version being looked at and published with it as the
 * parent, so the diff view has something to compare against straight away.
 * Publishing is content-addressed, so saving without changing anything is a
 * no-op rather than a duplicate version.
 */
function NewVersionForm({
  base,
  pending,
  error,
  onCancel,
  onSubmit,
}: {
  base: string;
  pending: boolean;
  error: unknown;
  onCancel: () => void;
  onSubmit: (input: { text: string; notes?: string; label?: string }) => void;
}) {
  const [text, setText] = React.useState(base);
  const [notes, setNotes] = React.useState('');
  const [promote, setPromote] = React.useState(false);

  return (
    <Card className="p-4">
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit({
            text,
            notes: notes || undefined,
            label: promote ? 'production' : undefined,
          });
        }}
      >
        <label className="flex flex-col gap-1 text-xs">
          <span className="text-muted-foreground">
            Prompt text. The version id is <code>sha256</code> of exactly this, so an unchanged
            save writes nothing.
          </span>
          <textarea
            value={text}
            onChange={(event) => setText(event.target.value)}
            rows={14}
            className="rounded-md border border-border bg-transparent p-3 font-mono text-xs leading-relaxed outline-none focus-visible:ring-2 focus-visible:ring-primary"
          />
        </label>
        <label className="flex flex-col gap-1 text-xs">
          <span className="text-muted-foreground">Notes — the commit message of a prompt.</span>
          <input
            value={notes}
            onChange={(event) => setNotes(event.target.value)}
            placeholder="say the model to read the dimension lines rather than estimate"
            className="h-9 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
          />
        </label>
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          <input
            type="checkbox"
            checked={promote}
            onChange={(event) => setPromote(event.target.checked)}
          />
          Make it production — storing a prompt and deploying it are different decisions, so this
          is opt-in.
        </label>
        {error ? <ErrorText error={error} /> : null}
        <div className="flex items-center gap-2">
          <Button type="submit" disabled={pending || !text.trim()}>
            {pending ? <Spinner /> : null}
            Publish version
          </Button>
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </form>
    </Card>
  );
}
