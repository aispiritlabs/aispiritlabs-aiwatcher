import * as React from 'react';
import { Link, createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Plus, Search } from 'lucide-react';
import { z } from 'zod';

import { listPrompts, publishPrompt } from '@/api/generated/sdk.gen';
import type { PromptSummary } from '@/api/generated/types.gen';
import { Badge, Button, Card, EmptyState, IdChip, Spinner } from '@/components/ui/primitives';
import { OutcomeBadge, SplitDeltas } from '@/components/prompt-bits';
import { RegistryDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import { formatTime } from '@/lib/utils';

/**
 * The prompts a system runs on, and what has been tried on them.
 *
 * The one area here that reads something other than the event log. Runs,
 * spans and evaluations are folds over a log with retention; a prompt is
 * authored, and the version a run used has to be readable long after that run
 * has been evicted. So it lives in an object store — RustFS in a deployment —
 * and this reads it over `/api/v1/prompts`.
 *
 * ## What the list is for
 *
 * Not "which prompts exist" — a repository answers that. It is "which prompts
 * have been optimised, and did any of it stick". So the row that carries the
 * most is the last optimisation: its dev gain beside its held-out gain, and
 * whether it was admitted. A registry of five prompts with twenty rejected
 * optimisations between them is a real finding, and it is one this page shows
 * without opening anything.
 */

const searchSchema = z.object({
  q: z.string().optional(),
  tag: z.string().optional(),
});

export const Route = createFileRoute('/prompts/')({
  validateSearch: searchSchema,
  component: PromptsPage,
});

function PromptsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const [creating, setCreating] = React.useState(false);

  // The input holds a draft and a debounce commits it to the URL, so a link to
  // a filtered list is shareable without a keystroke per history entry.
  const [draft, setDraft] = React.useState(search.q ?? '');
  React.useEffect(() => {
    const timer = setTimeout(() => {
      if ((search.q ?? '') === draft) return;
      void navigate({
        search: (previous) => ({ ...previous, q: draft || undefined }),
        replace: true,
      });
    }, 250);
    return () => clearTimeout(timer);
  }, [draft, navigate, search.q]);

  const prompts = useQuery({
    queryKey: ['prompts', search],
    queryFn: async () => {
      const response = await listPrompts({
        query: { search: search.q, tag: search.tag },
      });
      if (!response.data) throw response.error ?? new Error('failed to list prompts');
      return response.data;
    },
    retry: (count, error) => !isRegistryDisabled(error) && count < 2,
  });

  const create = useMutation({
    mutationFn: async (input: { name: string; text: string; description?: string }) => {
      const response = await publishPrompt({ body: input });
      if (!response.data) throw response.error ?? new Error('failed to publish');
      return response.data;
    },
    onSuccess: async (published) => {
      setCreating(false);
      await queryClient.invalidateQueries({ queryKey: ['prompts'] });
      void navigate({ to: '/prompts/$name', params: { name: published.version.name } });
    },
  });

  if (isRegistryDisabled(prompts.error)) return <RegistryDisabled />;

  const rows = prompts.data?.prompts ?? [];

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Prompts</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Versioned by content, so the same text is always the same version. Each one keeps what
            an optimiser tried on it and whether the held-out split agreed.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <label className="relative">
            <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <input
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              placeholder="Search name, description, tag"
              className="h-9 w-72 rounded-md border border-border bg-transparent pl-8 pr-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
            />
          </label>
          <Button onClick={() => setCreating((open) => !open)} className="gap-1.5">
            <Plus className="h-3.5 w-3.5" />
            New prompt
          </Button>
        </div>
      </div>

      {creating ? (
        <NewPromptForm
          pending={create.isPending}
          error={create.error}
          onCancel={() => setCreating(false)}
          onSubmit={(input) => create.mutate(input)}
        />
      ) : null}

      {prompts.isLoading ? (
        <div className="flex items-center gap-2 p-10 text-sm text-muted-foreground">
          <Spinner /> loading the registry…
        </div>
      ) : rows.length === 0 ? (
        <EmptyState
          title={search.q ? 'Nothing matches that' : 'No prompts yet'}
          hint={
            search.q
              ? 'The search runs on the server over the name, the description and the tags.'
              : 'Publish one from here, or from a service with the SDK: aiwatcher.prompts.publish(...). `just seed-prompts` puts a worked example in.'
          }
        />
      ) : (
        <Card className="overflow-hidden">
          <table className="w-full text-sm">
            <thead className="border-b border-border bg-muted/40 text-xs uppercase tracking-wide text-muted-foreground">
              <tr>
                <th className="px-4 py-2 text-left font-medium">Prompt</th>
                <th className="px-4 py-2 text-left font-medium">Production</th>
                <th className="px-4 py-2 text-left font-medium">Versions</th>
                <th className="px-4 py-2 text-left font-medium">Last optimisation</th>
                <th className="px-4 py-2 text-left font-medium">Updated</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((prompt) => (
                <PromptRow key={prompt.name} prompt={prompt} />
              ))}
            </tbody>
          </table>
        </Card>
      )}

      {prompts.data && prompts.data.total > rows.length ? (
        <p className="text-xs text-muted-foreground">
          Showing {rows.length} of {prompts.data.total} prompts.
        </p>
      ) : null}
    </div>
  );
}

function PromptRow({ prompt }: { prompt: PromptSummary }) {
  const last = prompt.last_optimization;
  return (
    <tr className="border-b border-border/60 last:border-0 hover:bg-accent/40">
      <td className="px-4 py-3 align-top">
        <Link
          to="/prompts/$name"
          params={{ name: prompt.name }}
          className="font-medium text-primary hover:underline"
        >
          {prompt.name}
        </Link>
        {prompt.description ? (
          <p className="mt-0.5 max-w-md text-xs text-muted-foreground">{prompt.description}</p>
        ) : null}
        {prompt.tags && prompt.tags.length > 0 ? (
          <div className="mt-1 flex flex-wrap gap-1">
            {prompt.tags.map((tag) => (
              <Badge key={tag}>{tag}</Badge>
            ))}
          </div>
        ) : null}
      </td>
      <td className="px-4 py-3 align-top">
        {prompt.labels?.production ? (
          <IdChip
            value={prompt.labels.production.slice(0, 12)}
            full={prompt.labels.production}
            label="production"
          />
        ) : (
          <span
            className="text-xs text-muted-foreground"
            title="Nothing has been promoted, so a reader gets the newest version."
          >
            unpinned
          </span>
        )}
      </td>
      <td className="px-4 py-3 align-top tabular-nums">
        {prompt.versions}
        {prompt.optimizations > 0 ? (
          <span className="ml-2 text-xs text-muted-foreground">
            {prompt.admitted_optimizations}/{prompt.optimizations} optimisations admitted
          </span>
        ) : null}
      </td>
      <td className="px-4 py-3 align-top">
        {last ? (
          <div className="flex flex-col gap-1">
            <div className="flex items-center gap-2">
              <OutcomeBadge record={last} />
              <span className="text-xs text-muted-foreground">{last.algorithm}</span>
            </div>
            <SplitDeltas record={last} />
          </div>
        ) : (
          <span className="text-xs text-muted-foreground">never optimised</span>
        )}
      </td>
      <td className="px-4 py-3 align-top text-xs tabular-nums text-muted-foreground">
        {formatTime(prompt.updated_at)}
      </td>
    </tr>
  );
}

/**
 * Publishing from the panel.
 *
 * A textarea and two fields rather than a dialog: Radix is not a dependency of
 * this panel yet, and the first thing that genuinely needs it is a select, not
 * this. The real write path is the SDK — a prompt normally arrives from the
 * service that uses it — and this exists so that starting a registry from an
 * empty screen does not require writing a script first.
 */
function NewPromptForm({
  pending,
  error,
  onCancel,
  onSubmit,
}: {
  pending: boolean;
  error: unknown;
  onCancel: () => void;
  onSubmit: (input: { name: string; text: string; description?: string }) => void;
}) {
  const [name, setName] = React.useState('');
  const [description, setDescription] = React.useState('');
  const [text, setText] = React.useState('');

  return (
    <Card className="p-4">
      <form
        className="flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit({ name, text, description: description || undefined });
        }}
      >
        <div className="grid gap-3 md:grid-cols-2">
          <label className="flex flex-col gap-1 text-xs">
            <span className="text-muted-foreground">
              Name — lowercase, no slashes. Namespace with dots: <code>planner.floor-plan</code>
            </span>
            <input
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="planner.floor-plan"
              className="h-9 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
            />
          </label>
          <label className="flex flex-col gap-1 text-xs">
            <span className="text-muted-foreground">Description</span>
            <input
              value={description}
              onChange={(event) => setDescription(event.target.value)}
              placeholder="What this prompt is for"
              className="h-9 rounded-md border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-primary"
            />
          </label>
        </div>
        <label className="flex flex-col gap-1 text-xs">
          <span className="text-muted-foreground">
            Prompt text. <code>{'{{ variables }}'}</code> are read from it, not declared.
          </span>
          <textarea
            required
            value={text}
            onChange={(event) => setText(event.target.value)}
            rows={10}
            className="rounded-md border border-border bg-transparent p-3 font-mono text-xs leading-relaxed outline-none focus-visible:ring-2 focus-visible:ring-primary"
          />
        </label>
        {error ? <ErrorText error={error} /> : null}
        <div className="flex items-center gap-2">
          <Button type="submit" disabled={pending || !name || !text.trim()}>
            {pending ? <Spinner /> : null}
            Publish
          </Button>
          <Button type="button" variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </form>
    </Card>
  );
}

export function ErrorText({ error }: { error: unknown }) {
  const body = error as { message?: string } | null;
  return (
    <p className="text-xs text-danger">
      {body?.message ?? 'The registry refused that. Check the name and the text.'}
    </p>
  );
}
