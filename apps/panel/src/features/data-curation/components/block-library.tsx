import * as React from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { BookOpen, Plus, Search } from 'lucide-react';

import { searchBlockLibrary, saveBlockTemplate } from '@/api/generated/sdk.gen';
import type { PipelineBlock, SaveBlockTemplateRequest } from '@/api/generated/types.gen';
import { Button, Card, Spinner } from '@/shared/components/ui/primitives';
import { createNotebook, getNotebook, getNotebookRevision } from '@/shared/lib/ml-pipeline';
import { answerOf } from '@/shared/lib/result';

type Props = {
  disabled: boolean;
  selected?: PipelineBlock;
  onAdd: (template: SaveBlockTemplateRequest) => void;
  onBusyChange: (busy: boolean) => void;
};

export function BlockLibrary({ disabled, selected, onAdd, onBusyChange }: Props) {
  const client = useQueryClient();
  const [open, setOpen] = React.useState(false);
  const [search, setSearch] = React.useState('');
  const [query, setQuery] = React.useState('');
  const [offset, setOffset] = React.useState(0);
  const [notice, setNotice] = React.useState('');
  const [publishTitle, setPublishTitle] = React.useState('');
  const [publishDescription, setPublishDescription] = React.useState('');
  React.useEffect(() => {
    const timer = setTimeout(() => {
      setQuery(search);
      setOffset(0);
    }, 250);
    return () => clearTimeout(timer);
  }, [search]);
  const library = useQuery({
    queryKey: ['curation-library', query, offset],
    enabled: open,
    queryFn: async () =>
      answerOf(
        await searchBlockLibrary({ query: { search: query, offset, limit: 12 } }),
        'Could not load the solutions library.',
      ),
  });
  const add = useMutation({
    mutationFn: async (template: SaveBlockTemplateRequest) => {
      const copy = structuredClone(template);
      if (copy.spec.kind === 'notebook') {
        const spec = copy.spec;
        if (!spec.revision) throw new Error('This solution has no pinned notebook source.');
        const source = await getNotebookRevision(spec.notebook, spec.revision);
        const notebook = await createNotebook(source.source);
        copy.spec = { ...spec, notebook: notebook.name, revision: notebook.revision };
      }
      return copy;
    },
    onSuccess: (copy) => {
      onAdd(copy);
      setNotice(`Added an editable copy of ${copy.title}.`);
    },
  });
  const publish = useMutation({
    mutationFn: async () => {
      if (!selected) throw new Error('Select a block to share.');
      const spec = structuredClone(selected.spec);
      if (spec.kind === 'notebook' && !spec.revision) {
        spec.revision = (await getNotebook(spec.notebook)).revision;
      }
      return answerOf(
        await saveBlockTemplate({
          body: {
            id: `solution-${crypto.randomUUID().slice(0, 20)}`,
            title: publishTitle.trim() || selected.title || 'Untitled block',
            description: publishDescription,
            tags: [],
            spec,
          },
        }),
        'Could not publish this solution.',
      );
    },
    onSuccess: () => {
      setNotice('Solution shared with everyone who can access this installation.');
      setPublishTitle('');
      setPublishDescription('');
      void client.invalidateQueries({ queryKey: ['curation-library'] });
    },
  });
  const busy = add.isPending || publish.isPending;
  React.useEffect(() => {
    onBusyChange(busy);
  }, [busy, onBusyChange]);

  return (
    <div className="w-full">
      <Button variant="outline" onClick={() => setOpen(!open)} aria-expanded={open}>
        <BookOpen className="h-4 w-4" /> Public solutions library
      </Button>
      {open ? (
        <Card className="mt-3 p-4" aria-label="Public solutions library">
          <p className="mb-3 text-xs text-muted-foreground">
            Shared solutions on this installation. Add a copy to your flow, then edit its code and
            parameters.
          </p>
          <label className="flex items-center gap-2">
            <Search className="h-4 w-4" />
            <input
              aria-label="Search public solutions"
              type="search"
              maxLength={256}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder="Search by title, task, tags or language…"
              className="h-9 w-full rounded-md border border-border bg-background px-3 text-sm"
            />
          </label>
          {library.isPending ? (
            <p className="mt-3 text-sm">
              <Spinner /> Loading solutions…
            </p>
          ) : library.isError ? (
            <p role="alert" className="mt-3 text-sm text-danger">
              {library.error.message}{' '}
              <Button variant="ghost" onClick={() => void library.refetch()}>
                Retry
              </Button>
            </p>
          ) : (
            <>
              <p className="my-3 text-xs text-muted-foreground">{library.data.total} solutions</p>
              <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                {library.data.templates.map((template) => (
                  <article key={template.id} className="rounded-md border border-border p-3">
                    <h3 className="text-sm font-medium">{template.title}</h3>
                    <p className="my-2 text-xs text-muted-foreground">{template.description}</p>
                    <p className="mb-2 text-[11px] text-muted-foreground">
                      {template.tags?.join(' · ')}
                    </p>
                    <details className="mb-2 text-xs">
                      <summary className="cursor-pointer">View code and settings</summary>
                      <SolutionCode spec={template.spec} />
                    </details>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={disabled || busy}
                      onClick={() => add.mutate(template)}
                    >
                      <Plus className="h-3 w-3" /> Add copy
                    </Button>
                  </article>
                ))}
              </div>
              {library.data.total === 0 ? (
                <p className="mt-3 text-sm text-muted-foreground">
                  No matching solutions. Try another search or share a block.
                </p>
              ) : null}
              {library.data.total > 12 ? (
                <div className="mt-3 flex gap-2">
                  <Button
                    variant="outline"
                    disabled={offset === 0}
                    onClick={() => setOffset(Math.max(0, offset - 12))}
                  >
                    Previous
                  </Button>
                  <Button
                    variant="outline"
                    disabled={offset + 12 >= library.data.total}
                    onClick={() => setOffset(offset + 12)}
                  >
                    Next
                  </Button>
                </div>
              ) : null}
            </>
          )}
          {selected ? (
            <details className="mt-4 border-t border-border pt-3 text-sm">
              <summary className="cursor-pointer">Share selected block: {selected.title}</summary>
              <p className="my-2 text-xs text-muted-foreground">
                Publishing makes this code and its parameters available to other users of this
                installation.
              </p>
              <input
                aria-label="Solution title"
                maxLength={120}
                value={publishTitle}
                onChange={(e) => setPublishTitle(e.target.value)}
                placeholder={selected.title}
                className="mr-2 rounded-md border border-border bg-background p-2"
              />
              <input
                aria-label="Solution description"
                maxLength={8192}
                value={publishDescription}
                onChange={(e) => setPublishDescription(e.target.value)}
                placeholder="What does this block do?"
                className="mr-2 rounded-md border border-border bg-background p-2"
              />
              <Button disabled={disabled || busy} onClick={() => publish.mutate()}>
                Publish solution
              </Button>
            </details>
          ) : null}
          {busy ? (
            <p className="mt-3 text-xs">
              <Spinner /> Preparing solution…
            </p>
          ) : null}
          {notice ? (
            <p role="status" className="mt-3 text-xs text-muted-foreground">
              {notice}
            </p>
          ) : null}
          {add.error || publish.error ? (
            <p role="alert" className="mt-3 text-xs text-danger">
              {(add.error || publish.error)?.message}
            </p>
          ) : null}
        </Card>
      ) : null}
    </div>
  );
}

function SolutionCode({ spec }: Pick<SaveBlockTemplateRequest, 'spec'>) {
  const [expanded, setExpanded] = React.useState(false);
  const notebook = useQuery({
    queryKey: [
      'ml-pipeline',
      'library-source',
      spec.kind === 'notebook' ? spec.notebook : '',
      spec.kind === 'notebook' ? spec.revision : '',
    ],
    enabled: expanded && spec.kind === 'notebook' && Boolean(spec.revision),
    queryFn: () => {
      if (spec.kind !== 'notebook' || !spec.revision) throw new Error('Missing source revision.');
      return getNotebookRevision(spec.notebook, spec.revision);
    },
  });
  return (
    <>
      <pre className="my-2 max-h-48 overflow-auto whitespace-pre-wrap text-[11px]">
        {spec.kind === 'transform' ? spec.steps : JSON.stringify(spec, null, 2)}
      </pre>
      {spec.kind === 'notebook' ? (
        <>
          <Button size="sm" variant="ghost" onClick={() => setExpanded(!expanded)}>
            Python source
          </Button>
          {expanded ? (
            notebook.isPending ? (
              <Spinner />
            ) : notebook.isError ? (
              <p role="alert">{notebook.error.message}</p>
            ) : (
              <pre className="max-h-64 overflow-auto whitespace-pre-wrap text-[11px]">
                {notebook.data.source}
              </pre>
            )
          ) : null}
        </>
      ) : null}
    </>
  );
}
