import { createFileRoute, Link } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { ShieldAlert } from 'lucide-react';
import { z } from 'zod';

import { getModel, listModels, setModelLabel } from '@/api/generated';
import { RegistryDisabled, isRegistryDisabled } from '@/components/registry-disabled';
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
} from '@/components/ui/primitives';
import { cn, shortId } from '@/lib/utils';

/**
 * The model registry: what a training run produced, and which version a service
 * loads next.
 *
 * This page is the reason the training module lives in aiwatcher rather than in
 * Weights & Biases. A version names the export it was trained on and the run
 * that produced it; an agent span names a model. From a floor plan coming back
 * with bad geometry, the path back to the labelled images is two clicks and
 * never leaves one system.
 *
 * The refusal is the part worth reading. A version with no held-out
 * measurement, or one trained on a dataset name nobody can reconstruct, cannot
 * take a label — and the reason is shown on the version rather than as a
 * disabled button with no explanation.
 */

const searchSchema = z.object({
  model: z.string().optional(),
  version: z.string().optional(),
});

export const Route = createFileRoute('/training/models')({
  validateSearch: searchSchema,
  component: ModelsPage,
});

function ModelsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();

  const models = useQuery({
    queryKey: ['models'],
    queryFn: async () => {
      const response = await listModels({ throwOnError: true });
      return response.data;
    },
    retry: false,
  });

  const name = search.model ?? models.data?.models[0]?.name;

  const detail = useQuery({
    queryKey: ['model', name, search.version],
    enabled: Boolean(name),
    queryFn: async () => {
      const response = await getModel({
        throwOnError: true,
        path: { name: name ?? '' },
        query: { version: search.version },
      });
      return response.data;
    },
  });

  const promote = useMutation({
    mutationFn: async (version: string) => {
      const response = await setModelLabel({
        throwOnError: true,
        path: { name: name ?? '' },
        body: { label: 'production', version },
      });
      return response.data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['model', name] });
      void queryClient.invalidateQueries({ queryKey: ['models'] });
    },
  });

  if (models.isError && isRegistryDisabled(models.error)) {
    return <RegistryDisabled area="Training" />;
  }
  if (models.isLoading) {
    return (
      <div className="flex justify-center p-10">
        <Spinner />
      </div>
    );
  }
  if (!name) {
    return (
      <EmptyState
        title="No models registered"
        hint="A finished training run registers one with TrainingClient.register_model."
      />
    );
  }

  const head = detail.data?.head;
  const current = detail.data?.current;
  const production = head?.labels?.production;

  return (
    <div className="grid gap-3 lg:grid-cols-[18rem_1fr]">
      <Card className="max-h-[calc(100vh-11rem)] overflow-y-auto p-1">
        <ul className="flex flex-col gap-0.5">
          {(models.data?.models ?? []).map((model) => (
            <li key={model.name}>
              <button
                type="button"
                onClick={() => navigate({ search: { model: model.name } })}
                className={cn(
                  'flex w-full flex-col gap-0.5 rounded-md px-2 py-1.5 text-left text-xs',
                  model.name === name ? 'bg-accent' : 'hover:bg-accent/50',
                )}
              >
                <span className="truncate font-medium">{model.name}</span>
                <span className="text-[10px] text-muted-foreground">
                  {model.versions?.length ?? 0} versions
                  {model.labels?.production ? ' · production set' : ' · nothing promoted'}
                </span>
              </button>
            </li>
          ))}
        </ul>
      </Card>

      <div className="flex flex-col gap-3">
        {current && (
          <Card>
            <CardHeader className="flex-row flex-wrap items-center gap-2">
              <CardTitle className="text-sm">{current.name}</CardTitle>
              <IdChip value={current.version} />
              {production === current.version && <Badge tone="success">production</Badge>}
              {!current.reproducible && <Badge tone="warning">not reproducible</Badge>}
            </CardHeader>
            <CardContent className="flex flex-col gap-3 text-xs">
              <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-muted-foreground">
                <dt>from run</dt>
                <dd>
                  <Link
                    to="/training/runs"
                    search={{ run: current.run_id }}
                    className="font-mono text-primary hover:underline"
                  >
                    {current.run_id}
                  </Link>
                </dd>
                <dt>dataset</dt>
                <dd className="font-mono text-foreground">{current.dataset}</dd>
                <dt>checkpoint</dt>
                <dd className="truncate font-mono text-foreground">{current.checkpoint_uri}</dd>
                <dt>code</dt>
                <dd className="font-mono text-foreground">{current.code || '—'}</dd>
              </dl>

              <Metrics metrics={current.metrics} />

              <div className="flex items-center gap-2">
                <Button
                  size="sm"
                  disabled={promote.isPending || production === current.version}
                  onClick={() => promote.mutate(current.version)}
                >
                  {production === current.version ? 'Live' : 'Promote to production'}
                </Button>
                {promote.isError && (
                  <span className="flex items-center gap-1 text-[11px] text-danger">
                    <ShieldAlert className="h-3.5 w-3.5" />
                    {(promote.error as { message?: string })?.message ?? 'refused'}
                  </span>
                )}
              </div>
              <p className="text-[11px] text-muted-foreground">
                Promotion needs a held-out measurement and an immutable dataset reference. A
                validation score is the number training selected against, so promoting on it
                promotes the selection.
              </p>
            </CardContent>
          </Card>
        )}

        <Card>
          <CardHeader>
            <CardTitle className="text-sm">Versions</CardTitle>
          </CardHeader>
          <CardContent className="overflow-x-auto">
            <table className="w-full text-xs">
              <thead className="text-muted-foreground">
                <tr>
                  <th className="py-1 text-left font-medium">version</th>
                  <th className="py-1 text-left font-medium">run</th>
                  <th className="py-1 text-left font-medium">dataset</th>
                  <th className="py-1 text-right font-medium">validation</th>
                  <th className="py-1 text-right font-medium">held-out</th>
                  <th className="py-1 text-right font-medium">gap</th>
                </tr>
              </thead>
              <tbody>
                {(head?.versions ?? []).map((version) => {
                  const metric =
                    Object.keys(version.metrics?.test ?? {})[0] ??
                    Object.keys(version.metrics?.validation ?? {})[0];
                  const validation = metric ? version.metrics?.validation?.[metric] : undefined;
                  const test = metric ? version.metrics?.test?.[metric] : undefined;
                  return (
                    <tr
                      key={version.version}
                      onClick={() =>
                        navigate({ search: { model: name, version: version.version } })
                      }
                      className={cn(
                        'cursor-pointer border-t border-border',
                        version.version === current?.version && 'bg-accent',
                      )}
                    >
                      <td className="py-1 font-mono">
                        {shortId(version.version, 12)}
                        {production === version.version && (
                          <Badge tone="success" className="ml-2 px-1.5 py-0 text-[10px]">
                            production
                          </Badge>
                        )}
                      </td>
                      <td className="py-1 font-mono">{version.run_id}</td>
                      <td className="py-1 font-mono">
                        {version.reproducible ? shortId(version.dataset, 28) : version.dataset}
                      </td>
                      <td className="py-1 text-right tabular-nums">
                        {validation?.toFixed(4) ?? '—'}
                      </td>
                      <td className="py-1 text-right tabular-nums">{test?.toFixed(4) ?? '—'}</td>
                      <td className="py-1 text-right tabular-nums text-muted-foreground">
                        {validation !== undefined && test !== undefined
                          ? (validation - test).toFixed(4)
                          : '—'}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            <p className="mt-2 text-[11px] text-muted-foreground">
              The gap is what to follow across a series. A version that gains on validation and
              not on held-out gained on the split its own selection ran against.
            </p>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function Metrics({
  metrics,
}: {
  metrics?: { validation?: Record<string, number>; test?: Record<string, number> } | null;
}) {
  const names = [
    ...new Set([
      ...Object.keys(metrics?.validation ?? {}),
      ...Object.keys(metrics?.test ?? {}),
    ]),
  ].sort();
  if (names.length === 0) {
    return (
      <p className="text-[11px] text-warning">
        No measurements recorded. Nothing can be promoted without a held-out one.
      </p>
    );
  }
  return (
    <div className="flex flex-wrap gap-4">
      {names.map((metric) => (
        <div key={metric} className="flex flex-col">
          <span className="text-[11px] text-muted-foreground">{metric}</span>
          <span className="tabular-nums">
            {metrics?.validation?.[metric]?.toFixed(4) ?? '—'}
            <span className="px-1 text-muted-foreground">/</span>
            <span className={metrics?.test?.[metric] === undefined ? 'text-warning' : ''}>
              {metrics?.test?.[metric]?.toFixed(4) ?? 'no held-out'}
            </span>
          </span>
        </div>
      ))}
    </div>
  );
}
