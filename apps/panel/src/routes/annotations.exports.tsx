import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Copy, Download } from 'lucide-react';
import { z } from 'zod';

import { buildExport, getExport, listExports, listProjects } from '@/api/generated';
import type { RightsPolicy, Split } from '@/api/generated/types.gen';
import { RegistryDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import {
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  EmptyState,
  Spinner,
  Stat,
} from '@/components/ui/primitives';
import { rejectionDetails } from '@/lib/annotations';

/**
 * Freezing a project into something a training run can name.
 *
 * The reference is `project@export-sha256`, and it is the same shape as
 * `dataset@version` in ADR_0015 for the same reason: a name alone is mutable,
 * and a training run that records only a name cannot prove what it was trained
 * on. Two exports of an unchanged project are one export, so building this
 * nightly costs nothing.
 *
 * The exclusion table is the part people skip and should not. An export that
 * quietly loses a third of a corpus reads exactly like one that did not, so
 * every image left out is listed with its reason.
 */

const searchSchema = z.object({
  project: z.string().optional(),
  export: z.string().optional(),
});

export const Route = createFileRoute('/annotations/exports')({
  validateSearch: searchSchema,
  component: ExportsPage,
});

const POLICY_NOTES: Record<RightsPolicy, string> = {
  commercial: 'Only owned or commercially licensed images. Everything else is listed as excluded.',
  research: 'Adds research-only corpora such as CubiCasa5K. The manifest records that it did.',
  any: 'Includes images nobody has checked. For an experiment whose weights are thrown away.',
};

function ExportsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const [policy, setPolicy] = React.useState<RightsPolicy>('commercial');
  const [requireHuman, setRequireHuman] = React.useState(true);
  const [note, setNote] = React.useState('');

  const projects = useQuery({
    queryKey: ['annotation-projects'],
    queryFn: async () => {
      const response = await listProjects({ throwOnError: true });
      return response.data;
    },
    retry: false,
  });

  const projectName = search.project ?? projects.data?.projects[0]?.name;

  const exports = useQuery({
    queryKey: ['annotation-exports', projectName],
    enabled: Boolean(projectName),
    queryFn: async () => {
      const response = await listExports({
        throwOnError: true,
        query: { name: projectName ?? '' },
      });
      return response.data;
    },
  });

  const selectedId = search.export ?? exports.data?.exports[0]?.export;

  const manifest = useQuery({
    queryKey: ['annotation-export', projectName, selectedId],
    enabled: Boolean(projectName && selectedId),
    queryFn: async () => {
      const response = await getExport({
        throwOnError: true,
        query: { project: projectName ?? '', export: selectedId ?? '' },
      });
      return response.data;
    },
  });

  const build = useMutation({
    mutationFn: async () => {
      const response = await buildExport({
        throwOnError: true,
        body: {
          project: projectName ?? '',
          note,
          rights_policy: policy,
          require_human_review: requireHuman,
        },
      });
      return response.data;
    },
    onSuccess: (built) => {
      void queryClient.invalidateQueries({ queryKey: ['annotation-exports', projectName] });
      navigate({ search: (previous) => ({ ...previous, export: built.manifest.export }) });
    },
  });

  // Every hook above this line, and every early return below it. A conditional
  // return placed between them changes the hook count between renders, which
  // React notices as "rendered fewer hooks than expected" the moment a
  // disabled registry comes back.
  if (projects.isError && isRegistryDisabled(projects.error)) {
    return <RegistryDisabled area="Annotations" />;
  }
  if (projects.isLoading) {
    return (
      <div className="flex justify-center p-10">
        <Spinner />
      </div>
    );
  }
  if (!projectName) {
    return <EmptyState title="No annotation project yet" hint="Create one on the Label tab." />;
  }

  const base = import.meta.env.VITE_API_BASE_URL ?? '';
  const cocoUrl = (split?: Split) =>
    `${base}/api/v1/annotation-export/coco?project=${encodeURIComponent(projectName)}&export=${selectedId}${
      split ? `&split=${split}` : ''
    }`;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Training exports</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            An immutable manifest of which images, on which side of the split, at which accepted
            revision. Its id is the string a training run records.
          </p>
        </div>
        <select
          value={projectName}
          onChange={(event) =>
            navigate({ search: { project: event.target.value, export: undefined } })
          }
          className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
        >
          {(projects.data?.projects ?? []).map((project) => (
            <option key={project.name} value={project.name}>
              {project.name}
            </option>
          ))}
        </select>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-sm">Build an export</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-wrap items-end gap-3 text-xs">
          <label className="flex flex-col gap-1">
            <span className="font-medium text-muted-foreground">rights policy</span>
            <select
              value={policy}
              onChange={(event) => setPolicy(event.target.value as RightsPolicy)}
              className="rounded-md border border-border bg-background px-2 py-1.5"
            >
              <option value="commercial">commercial</option>
              <option value="research">research</option>
              <option value="any">any</option>
            </select>
          </label>
          <label className="flex items-center gap-2 pb-2">
            <input
              type="checkbox"
              checked={requireHuman}
              onChange={(event) => setRequireHuman(event.target.checked)}
            />
            <span>a human drew or corrected it</span>
          </label>
          <label className="flex flex-1 flex-col gap-1">
            <span className="font-medium text-muted-foreground">note</span>
            <input
              value={note}
              onChange={(event) => setNote(event.target.value)}
              placeholder="what this cut is for"
              className="rounded-md border border-border bg-background px-2 py-1.5"
            />
          </label>
          <Button size="sm" onClick={() => build.mutate()} disabled={build.isPending}>
            Build
          </Button>
          <p className="w-full text-[11px] text-muted-foreground">{POLICY_NOTES[policy]}</p>
          {build.isError && (
            <p className="w-full text-[11px] text-danger">{rejectionDetails(build.error)[0]}</p>
          )}
        </CardContent>
      </Card>

      <div className="grid gap-3 lg:grid-cols-[18rem_1fr]">
        <Card className="max-h-[36rem] overflow-y-auto p-1">
          {(exports.data?.exports ?? []).length === 0 ? (
            <EmptyState title="No exports yet" hint="Build one above." />
          ) : (
            <ul className="flex flex-col gap-0.5">
              {(exports.data?.exports ?? []).map((summary) => (
                <li key={summary.export}>
                  <button
                    type="button"
                    onClick={() =>
                      navigate({ search: (previous) => ({ ...previous, export: summary.export }) })
                    }
                    className={`flex w-full flex-col gap-0.5 rounded-md px-2 py-1.5 text-left text-xs ${
                      summary.export === selectedId ? 'bg-accent' : 'hover:bg-accent/50'
                    }`}
                  >
                    <span className="font-mono">{summary.export.slice(0, 12)}</span>
                    <span className="text-[10px] text-muted-foreground">
                      {summary.counts.images} images · {summary.counts.instances} instances ·{' '}
                      {summary.rights_policy}
                    </span>
                    {summary.note && (
                      <span className="truncate text-[10px] text-muted-foreground">
                        {summary.note}
                      </span>
                    )}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </Card>

        {manifest.data ? (
          <div className="flex flex-col gap-3">
            <Card>
              <CardContent className="flex flex-wrap items-center gap-3 pt-4">
                <code className="rounded bg-muted px-2 py-1 font-mono text-xs">
                  {manifest.data.project}@{manifest.data.export}
                </code>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() =>
                    void navigator.clipboard?.writeText(
                      `${manifest.data.project}@${manifest.data.export}`,
                    )
                  }
                >
                  <Copy className="h-3.5 w-3.5" /> Copy reference
                </Button>
                <span className="text-xs text-muted-foreground">
                  Put this in <code>train.started.data.dataset</code>.
                </span>
                <div className="ml-auto flex gap-2">
                  {(['train', 'validation', 'test'] as const).map((split) => (
                    <a
                      key={split}
                      href={cocoUrl(split)}
                      target="_blank"
                      rel="noreferrer noopener"
                      className="inline-flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-accent"
                    >
                      <Download className="h-3 w-3" /> {split} COCO
                    </a>
                  ))}
                </div>
              </CardContent>
            </Card>

            <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
              <Stat label="Images" value={String(manifest.data.counts.images)} />
              <Stat label="Families" value={String(manifest.data.counts.groups)} />
              <Stat label="Instances" value={String(manifest.data.counts.instances)} />
              <Stat label="Excluded" value={String(manifest.data.counts.excluded)} />
            </div>

            <Card>
              <CardHeader>
                <CardTitle className="text-sm">Instances per class and split</CardTitle>
              </CardHeader>
              <CardContent className="overflow-x-auto">
                <table className="w-full text-xs">
                  <thead className="text-muted-foreground">
                    <tr>
                      <th className="py-1 text-left font-medium">class</th>
                      <th className="py-1 text-right font-medium">train</th>
                      <th className="py-1 text-right font-medium">val</th>
                      <th className="py-1 text-right font-medium">test</th>
                      <th className="py-1 text-right font-medium">total</th>
                    </tr>
                  </thead>
                  <tbody>
                    {Object.entries(manifest.data.counts.instances_per_class).map(
                      ([className, total]) => {
                        const perSplit =
                          manifest.data.counts.instances_per_class_split[className] ?? {};
                        return (
                          <tr key={className} className="border-t border-border">
                            <td className="py-1 font-mono">{className}</td>
                            <td className="py-1 text-right font-mono">{perSplit.train ?? 0}</td>
                            <td className="py-1 text-right font-mono">
                              {perSplit.validation ?? 0}
                            </td>
                            <td className="py-1 text-right font-mono">{perSplit.test ?? 0}</td>
                            <td className="py-1 text-right font-mono">{total}</td>
                          </tr>
                        );
                      },
                    )}
                  </tbody>
                </table>
                <p className="mt-2 text-[11px] text-muted-foreground">
                  A test column in single digits invalidates a recall figure before anybody quotes
                  it. This table is here to be read before the metric is.
                </p>
              </CardContent>
            </Card>

            {manifest.data.excluded.length > 0 && (
              <Card>
                <CardHeader>
                  <CardTitle className="text-sm">
                    Excluded ({manifest.data.excluded.length})
                  </CardTitle>
                </CardHeader>
                <CardContent className="flex max-h-72 flex-col gap-1 overflow-y-auto text-xs">
                  {manifest.data.excluded.map((exclusion) => (
                    <div
                      key={exclusion.image_id}
                      className="flex items-center gap-2 border-b border-border py-1 last:border-0"
                    >
                      <Badge tone="warning" className="px-1.5 py-0 text-[10px]">
                        {exclusion.reason.replace('_', ' ')}
                      </Badge>
                      <span className="font-mono text-[11px]">{exclusion.group_id}</span>
                      <span className="truncate text-[11px] text-muted-foreground">
                        {exclusion.detail}
                      </span>
                    </div>
                  ))}
                </CardContent>
              </Card>
            )}
          </div>
        ) : (
          <Card className="flex items-center justify-center p-10">
            {manifest.isLoading ? <Spinner /> : <EmptyState title="Pick an export" />}
          </Card>
        )}
      </div>
    </div>
  );
}
