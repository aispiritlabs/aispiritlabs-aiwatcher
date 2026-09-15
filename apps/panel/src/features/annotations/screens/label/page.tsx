import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { getRouteApi, useBlocker } from '@tanstack/react-router';
import {
  ArrowLeft,
  ArrowRight,
  Check,
  MousePointer2,
  PenLine,
  Redo2,
  Undo2,
  Upload,
} from 'lucide-react';
import * as React from 'react';
import { z } from 'zod';
import { searchSchema } from './search';

import {
  getImage,
  getProject,
  listImages,
  listProjects,
  registerImage,
  reviewImage,
  saveProject,
  saveRevision,
  uploadBlob,
} from '@/api/generated';
import type {
  Annotation,
  Geometry,
  ImageHead,
  ImagePage,
  LabelClass,
  ReviewState,
  Split,
  UsageRights,
} from '@/api/generated/types.gen';
import { AnnotationCanvas, type Tool } from '@/features/annotations/components/annotation-canvas';
import {
  ClassPalette,
  ShapeInspector,
  ShapeList,
} from '@/features/annotations/components/annotation-inspector';
import {
  REVIEW_TONES,
  SPLIT_LABELS,
  classOf,
  defaultAttributes,
  nextId,
} from '@/features/annotations/lib/annotations';
import { useAnnotationHistory } from '@/features/annotations/lib/annotation-history';
import { VirtualList } from '@/shared/components/virtual-list';
import { rejectionDetails } from '@/shared/lib/rejection';
import { RegistryDisabled, isRegistryDisabled } from '@/shared/components/registry-disabled';
import { Badge, Button, Card, EmptyState, Spinner } from '@/shared/components/ui/primitives';
import { cn } from '@/shared/lib/utils';

const routeApi = getRouteApi('/annotations/label');
const imageContext = (image: string | undefined, revision: string | undefined) =>
  JSON.stringify([image, revision]);
function uniqueImages(pages: ImagePage[]): ImageHead[] {
  const found = new Map<string, ImageHead>();
  for (const page of pages) for (const head of page.images) found.set(head.image.image_id, head);
  return [...found.values()];
}

/** `aiwatcher://blob/<sha>` → the route that serves those bytes. */
function imageSrc(uri: string): string {
  const prefix = 'aiwatcher://blob/';
  const base = import.meta.env.VITE_API_BASE_URL ?? '';
  return uri.startsWith(prefix)
    ? `${base}/api/v1/annotation-blobs/${uri.slice(prefix.length)}`
    : uri;
}

export function LabelPage() {
  const search = routeApi.useSearch();
  const navigate = routeApi.useNavigate();
  const queryClient = useQueryClient();

  const projects = useQuery({
    queryKey: ['annotation-projects'],
    queryFn: async () => {
      const response = await listProjects({ throwOnError: true });
      return response.data;
    },
    retry: false,
  });

  const available = projects.data?.projects ?? [];
  const projectName = search.project ?? available[0]?.name;

  // Every hook is above this line. A conditional return between two of them
  // changes the hook count between renders, which React notices the moment a
  // disabled registry comes back.
  if (projects.isError && isRegistryDisabled(projects.error)) {
    return <RegistryDisabled area="Annotations" />;
  }
  if (projects.isError) {
    return (
      <EmptyState
        title="Could not load annotation projects"
        hint="The request failed. Reload to try again; existing projects have not been removed."
      />
    );
  }
  if (projects.isLoading) {
    return (
      <div className="flex justify-center p-10">
        <Spinner />
      </div>
    );
  }
  if (!projectName) {
    return <FirstProject onCreated={(name) => navigate({ search: { project: name } })} />;
  }

  return (
    <Workspace
      key={projectName}
      projectName={projectName}
      projects={available.map((project) => project.name)}
      search={search}
      onSearch={(next, options) =>
        navigate({
          ...options,
          search: (previous) => ({
            ...previous,
            ...('image' in next || 'project' in next ? { revision: undefined } : {}),
            ...next,
          }),
        })
      }
      onInvalidate={() => {
        void queryClient.invalidateQueries({ queryKey: ['annotation-images', projectName] });
      }}
    />
  );
}

function Workspace({
  projectName,
  projects,
  search,
  onSearch,
  onInvalidate,
}: {
  projectName: string;
  projects: string[];
  search: z.infer<typeof searchSchema>;
  onSearch: (next: Partial<z.infer<typeof searchSchema>>, options?: { replace?: boolean }) => void;
  onInvalidate: () => void;
}) {
  const queryClient = useQueryClient();
  const [tool, setTool] = React.useState<Tool>('select');
  const [activeClass, setActiveClass] = React.useState<string>('');
  const history = useAnnotationHistory();
  const [canvasDraft, setCanvasDraft] = React.useState(false);
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [linking, setLinking] = React.useState<string | null>(null);
  const [problems, setProblems] = React.useState<string[]>([]);
  const savedRevision = React.useRef<string | undefined>(undefined);

  const project = useQuery({
    queryKey: ['annotation-project', projectName],
    queryFn: async () => {
      const response = await getProject({ throwOnError: true, query: { name: projectName } });
      return response.data;
    },
  });

  const images = useInfiniteQuery({
    queryKey: ['annotation-images', projectName, search.review, search.split, search.q],
    initialPageParam: 0,
    getNextPageParam: (page: ImagePage) => page.next_offset ?? undefined,
    queryFn: async ({ pageParam }) => {
      const response = await listImages({
        throwOnError: true,
        query: {
          project: projectName,
          review: search.review,
          split: search.split,
          search: search.q || undefined,
          limit: 50,
          offset: pageParam,
        },
      });
      return response.data;
    },
  });

  const heads = React.useMemo(() => uniqueImages(images.data?.pages ?? []), [images.data]);
  const imageId = search.image ?? heads[0]?.image.image_id;
  const context = imageContext(imageId, search.revision);
  const navigationContext = JSON.stringify([context, search.q, search.review, search.split]);
  const activeNavigation = React.useRef<string | null>(navigationContext);
  React.useEffect(() => {
    activeNavigation.current = navigationContext;
    return () => {
      activeNavigation.current = null;
    };
  }, [navigationContext]);

  // Pin the first selection so filtering/refetching never silently changes the canvas.
  React.useEffect(() => {
    if (!search.image && imageId)
      onSearch({ project: projectName, image: imageId }, { replace: true });
  }, [search.image, imageId, projectName, onSearch]);
  const currentIndex = heads.findIndex((head) => head.image.image_id === imageId);
  const previousImage = currentIndex > 0 ? heads[currentIndex - 1] : undefined;
  const nextImage = currentIndex >= 0 ? heads[currentIndex + 1] : undefined;
  const goNext = async () => {
    if (nextImage) {
      onSearch({ image: nextImage.image.image_id });
      return;
    }
    if (!images.hasNextPage || currentIndex < 0 || images.isFetching) return;
    const result = await images.fetchNextPage();
    if (activeNavigation.current !== navigationContext || result.isError) return;
    const loaded = uniqueImages(result.data?.pages ?? []);
    const next = loaded[loaded.findIndex((head) => head.image.image_id === imageId) + 1];
    if (next) onSearch({ image: next.image.image_id });
  };

  const detail = useQuery({
    queryKey: ['annotation-image', projectName, imageId, search.revision],
    enabled: Boolean(imageId),
    queryFn: async () => {
      const response = await getImage({
        throwOnError: true,
        query: { project: projectName, image_id: imageId ?? '', revision: search.revision },
      });
      return response.data;
    },
  });

  const classes: LabelClass[] = project.data?.schema.classes ?? [];

  React.useEffect(() => {
    if (detail.data) history.load(context, detail.data.revision?.annotations ?? []);
  }, [context, detail.data, history.load]);
  React.useEffect(() => {
    savedRevision.current = undefined;
    setSelectedId(null);
    setLinking(null);
    setProblems([]);
    setTool('select');
    setCanvasDraft(false);
  }, [imageId, search.revision]);

  React.useEffect(() => {
    if (!activeClass && classes[0]) setActiveClass(classes[0].name);
  }, [activeClass, classes]);

  const ready = history.context === context && Boolean(detail.data) && !detail.isError;
  const annotations = ready ? history.present : [];
  const setDraft = history.change;
  const selected = annotations.find((annotation) => annotation.id === selectedId) ?? null;

  const save = useMutation({
    mutationFn: async (accept: boolean) => {
      const response = await saveRevision({
        throwOnError: true,
        body: {
          project: projectName,
          image_id: imageId ?? '',
          annotations,
          accept,
          notes: '',
        },
      });
      return response.data;
    },
    onSuccess: (saved, accept) => {
      setProblems([]);
      if (accept) {
        // Acceptance completes the labelling gesture, not only the registry
        // write. Return to navigation with no handles left active so the next
        // click cannot accidentally start or edit another shape.
        setTool('select');
        setSelectedId(null);
        setLinking(null);
      }
      queryClient.setQueryData(
        ['annotation-image', projectName, imageId, saved.revision.revision],
        {
          ...detail.data,
          ...saved.head,
          revision: saved.revision,
        },
      );
      savedRevision.current = saved.revision.revision;
      history.markSaved(
        blocker.status === 'blocked' ? context : imageContext(imageId, saved.revision.revision),
        saved.revision.annotations,
      );
      if (blocker.status !== 'blocked')
        onSearch({ image: imageId, revision: saved.revision.revision });
      onInvalidate();
    },
    onError: (error) => setProblems(rejectionDetails(error)),
  });

  const review = useMutation({
    mutationFn: async (state: ReviewState) => {
      const response = await reviewImage({
        throwOnError: true,
        body: {
          project: projectName,
          image_id: imageId ?? '',
          review: state,
          revision: state === 'accepted' ? (detail.data?.revision?.revision ?? null) : null,
          note: '',
        },
      });
      return response.data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['annotation-image', projectName, imageId] });
      onInvalidate();
    },
    onError: (error) => setProblems(rejectionDetails(error)),
  });

  const create = (geometry: Geometry) => {
    const definition = classOf(classes, activeClass);
    if (!definition) return;
    const annotation: Annotation = {
      id: nextId(annotations, activeClass),
      class: activeClass,
      geometry,
      attributes: defaultAttributes(definition),
      links: {},
      origin: 'human',
    };
    setDraft([...annotations, annotation]);
    setSelectedId(annotation.id);
    // Back to select after one shape: the alternative is a stray click on the
    // plan becoming a second wall nobody noticed.
    setTool('select');
  };

  const replace = (id: string, next: Annotation) =>
    setDraft(annotations.map((annotation) => (annotation.id === id ? next : annotation)));

  const onCanvasSelect = (id: string | null) => {
    if (linking && id && selected) {
      const existing = ((selected.links?.[linking] as string[] | undefined) ?? []).filter(
        (target) => target !== id,
      );
      const definition = classOf(classes, selected.class);
      const max = definition?.links?.find((link) => link.name === linking)?.max ?? 1;
      replace(selected.id, {
        ...selected,
        links: { ...(selected.links ?? {}), [linking]: [...existing, id].slice(-max) },
      });
      setLinking(null);
      return;
    }
    setSelectedId(id);
  };

  const counts = React.useMemo(() => {
    const out: Record<string, number> = {};
    for (const annotation of annotations) {
      out[annotation.class] = (out[annotation.class] ?? 0) + 1;
    }
    return out;
  }, [annotations]);

  const invalid = React.useMemo(() => {
    // The registry reports `annotation_id: what is wrong`. Matching the prefix
    // is enough to paint the shape red, and the sentence itself is shown in
    // full below the canvas.
    const ids = new Set<string>();
    for (const problem of problems) {
      const id = problem.split(':')[0]?.trim();
      if (id && annotations.some((annotation) => annotation.id === id)) ids.add(id);
    }
    return ids;
  }, [annotations, problems]);

  const dirty = (history.context === context && history.dirty) || canvasDraft;
  const blocker = useBlocker({
    shouldBlockFn: ({ next }) => {
      if (!dirty && !save.isPending && !review.isPending) return false;
      const target = next.search as { image?: string; project?: string; revision?: string };
      return (
        next.pathname !== '/annotations/label' ||
        (target.project ?? projectName) !== projectName ||
        (target.image ?? imageId) !== imageId ||
        (target.revision !== search.revision &&
          !(savedRevision.current && target.revision === savedRevision.current))
      );
    },
    enableBeforeUnload: dirty || save.isPending || review.isPending,
    withResolver: true,
  });

  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
      if (event.altKey) return;
      const key = event.key.toLowerCase();
      const command = event.metaKey || event.ctrlKey;
      if (command && ['s', 'z', 'y'].includes(key)) event.preventDefault();
      if (save.isPending || review.isPending || !ready || blocker.status === 'blocked') return;
      if (history.gesture) return;
      if (command) {
        if (canvasDraft || history.gesture) return;
        if (key === 'z') {
          event.shiftKey ? history.redo() : history.undo();
          setSelectedId(null);
          setLinking(null);
          setProblems([]);
        } else if (key === 'y') {
          history.redo();
          setSelectedId(null);
          setLinking(null);
          setProblems([]);
        } else if (key === 's') save.mutate(false);
        return;
      }
      if (event.key >= '1' && event.key <= '9') {
        const definition = classes[Number(event.key) - 1];
        if (definition) {
          setActiveClass(definition.name);
          setTool('draw');
        }
      } else if (event.key === 'd') {
        setTool('draw');
      } else if (event.key === 'v') {
        setTool('select');
      } else if (
        event.key.toLowerCase() === 'a' &&
        !event.metaKey &&
        !event.ctrlKey &&
        !event.altKey &&
        imageId &&
        !save.isPending &&
        !canvasDraft &&
        !history.gesture
      ) {
        event.preventDefault();
        save.mutate(true);
      } else if ((event.key === 'Delete' || event.key === 'x') && selectedId) {
        setDraft(annotations.filter((annotation) => annotation.id !== selectedId));
        setSelectedId(null);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [
    annotations,
    classes,
    imageId,
    save,
    review.isPending,
    selectedId,
    ready,
    canvasDraft,
    history,
    blocker.status,
  ]);

  return (
    <div
      className="flex flex-col gap-3"
      inert={save.isPending || review.isPending}
      aria-busy={save.isPending || review.isPending}
    >
      {blocker.status === 'blocked' && (
        <div
          role="alert"
          className="flex flex-wrap items-center gap-2 rounded border border-warning p-3 text-sm"
        >
          <span className="mr-auto">This image has unsaved changes.</span>
          <Button
            disabled={save.isPending || canvasDraft || Boolean(history.gesture) || !ready}
            onClick={() => {
              void save
                .mutateAsync(false)
                .then(() => blocker.proceed())
                .catch(() => {});
            }}
          >
            Save draft & leave
          </Button>
          <Button variant="outline" disabled={save.isPending} onClick={blocker.proceed}>
            Discard & leave
          </Button>
          <Button variant="ghost" onClick={blocker.reset}>
            Keep editing
          </Button>
        </div>
      )}
      {canvasDraft && (
        <p role="status" className="text-xs text-warning">
          Finish or cancel the current shape before saving. Leaving discards the unfinished shape.
        </p>
      )}
      {search.revision && (
        <p role="status" className="text-xs text-muted-foreground">
          Viewing saved revision <span className="font-mono">{search.revision}</span>. Acceptance is
          a separate review action.
        </p>
      )}
      <header className="flex flex-wrap items-center gap-2">
        <select
          aria-label="Annotation project"
          value={projectName}
          onChange={(event) => onSearch({ project: event.target.value, image: undefined })}
          className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
        >
          {projects.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
        {project.data && (
          <span className="text-xs text-muted-foreground">
            {project.data.accepted}/{project.data.images} accepted · {project.data.groups} families
            · {project.data.instances} instances
          </span>
        )}
        {/* `relative`, because the import panel below is absolutely positioned
            and would otherwise anchor to the page rather than to this row. */}
        <div className="relative ml-auto flex items-center gap-2">
          <ImageImport
            project={projectName}
            onImported={(head) => {
              onInvalidate();
              onSearch({ image: head.image.image_id });
            }}
          />
        </div>
      </header>

      <div className="grid gap-3 lg:grid-cols-[16rem_1fr_18rem]">
        <Card className="flex h-72 flex-col overflow-hidden lg:h-auto lg:max-h-[calc(100vh-12rem)]">
          <div className="flex shrink-0 flex-col gap-2 border-b border-border p-2">
            <input
              aria-label="Search images"
              value={search.q ?? ''}
              onChange={(event) => onSearch({ q: event.target.value || undefined })}
              placeholder="source, family, level…"
              className="rounded-md border border-border bg-background px-2 py-1 text-xs"
            />
            <div className="flex gap-1">
              <FilterSelect
                value={search.review}
                onChange={(value) => onSearch({ review: value as ReviewState | undefined })}
                options={['draft', 'in_review', 'accepted', 'rejected']}
                placeholder="review"
              />
              <FilterSelect
                value={search.split}
                onChange={(value) => onSearch({ split: value as Split | undefined })}
                options={['train', 'validation', 'test']}
                placeholder="split"
              />
            </div>
          </div>
          {images.isLoading ? (
            <p role="status" className="p-3 text-xs">
              Loading images…
            </p>
          ) : null}
          {images.isError ? (
            <div role="alert" className="p-2 text-xs text-danger">
              Could not load images.{' '}
              <Button
                size="sm"
                variant="outline"
                onClick={() =>
                  void (images.isFetchNextPageError ? images.fetchNextPage() : images.refetch())
                }
              >
                Retry images
              </Button>
            </div>
          ) : null}
          <VirtualList
            key={JSON.stringify([search.q, search.review, search.split])}
            items={heads}
            estimateSize={56}
            scrollToIndex={currentIndex >= 0 ? currentIndex : undefined}
            className="min-h-0 flex-1 p-1"
            keyOf={(head) => head.image.image_id}
            renderRow={(head) => (
              <button
                type="button"
                aria-label={`Open image ${head.image.group_id} (${head.image.image_id})`}
                aria-current={head.image.image_id === imageId ? 'true' : undefined}
                onClick={() => {
                  if (head.image.image_id !== imageId) onSearch({ image: head.image.image_id });
                }}
                className={cn(
                  'flex w-full items-center gap-2 rounded-md p-1 text-left text-xs',
                  head.image.image_id === imageId ? 'bg-accent' : 'hover:bg-accent/50',
                )}
              >
                <img
                  src={imageSrc(head.image.uri)}
                  alt=""
                  loading="lazy"
                  className="h-10 w-10 shrink-0 rounded border border-border object-cover"
                />
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-medium">{head.image.group_id}</span>
                  <span className="block truncate text-[10px] text-muted-foreground">
                    {head.image.level ?? head.image.source}
                  </span>
                </span>
                <Badge
                  tone={REVIEW_TONES[head.review ?? 'draft']}
                  className="px-1.5 py-0 text-[10px]"
                >
                  {(head.review ?? 'draft').replace('_', ' ')}
                </Badge>
              </button>
            )}
          />
          {images.data && heads.length === 0 && (
            <EmptyState
              title={
                search.q || search.review || search.split
                  ? 'No images match these filters'
                  : 'No images yet'
              }
              hint={
                search.q || search.review || search.split
                  ? 'Change the search or filters. The selected image remains open.'
                  : 'Add one with Import, or register a URL through the API.'
              }
            />
          )}
          <div className="flex shrink-0 flex-wrap items-center gap-2 border-t border-border p-2 text-xs">
            <span role="status">
              {heads.length} loaded · {images.data?.pages[0]?.total ?? 0} matching
            </span>
            {images.hasNextPage ? (
              <Button
                size="sm"
                variant="outline"
                disabled={images.isFetching}
                onClick={() => void images.fetchNextPage()}
              >
                {images.isFetchingNextPage ? 'Loading…' : 'Load more images'}
              </Button>
            ) : null}
          </div>
        </Card>

        <div className="flex min-w-0 flex-col gap-2">
          <nav aria-label="Image navigation" className="flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={!previousImage}
              onClick={() => previousImage && onSearch({ image: previousImage.image.image_id })}
            >
              <ArrowLeft className="h-3.5 w-3.5" /> Previous image
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={
                images.isFetching || (!nextImage && !(currentIndex >= 0 && images.hasNextPage))
              }
              onClick={() => void goNext()}
            >
              Next image <ArrowRight className="h-3.5 w-3.5" />
            </Button>
            <span className="min-w-0 break-all text-xs text-muted-foreground">
              {imageId
                ? `${detail.data?.image.group_id ?? imageId}${currentIndex >= 0 ? ` · ${currentIndex + 1} of ${images.data?.pages[0]?.total ?? heads.length}` : ' · outside the loaded results'}`
                : 'No image selected'}
            </span>
          </nav>
          <div className="flex flex-wrap items-center gap-2">
            <div className="flex overflow-hidden rounded-md border border-border">
              <ToolButton active={tool === 'select'} onClick={() => setTool('select')} hotkey="v">
                <MousePointer2 className="h-3.5 w-3.5" /> Select
              </ToolButton>
              <ToolButton active={tool === 'draw'} onClick={() => setTool('draw')} hotkey="d">
                <PenLine className="h-3.5 w-3.5" /> Draw
              </ToolButton>
            </div>
            <Button
              size="sm"
              variant="outline"
              aria-label="Undo annotation change"
              title="Undo (Ctrl/Cmd+Z)"
              disabled={!ready || !history.canUndo || canvasDraft}
              onClick={() => {
                history.undo();
                setSelectedId(null);
                setLinking(null);
                setProblems([]);
              }}
            >
              <Undo2 className="h-3.5 w-3.5" /> Undo
            </Button>
            <Button
              size="sm"
              variant="outline"
              aria-label="Redo annotation change"
              title="Redo (Ctrl/Cmd+Shift+Z)"
              disabled={!ready || !history.canRedo || canvasDraft}
              onClick={() => {
                history.redo();
                setSelectedId(null);
                setLinking(null);
                setProblems([]);
              }}
            >
              <Redo2 className="h-3.5 w-3.5" /> Redo
            </Button>
            {detail.data && (
              <>
                <Badge tone="neutral" className="px-2 py-0.5 text-[11px]">
                  {SPLIT_LABELS[detail.data.split]}
                </Badge>
                <Badge
                  tone={REVIEW_TONES[detail.data.review ?? 'draft']}
                  className="px-2 py-0.5 text-[11px]"
                >
                  {(detail.data.review ?? 'draft').replace('_', ' ')}
                </Badge>
                <span className="text-[11px] text-muted-foreground">
                  {detail.data.image.width}×{detail.data.image.height} ·{' '}
                  {detail.data.image.rights.kind.replace('_', ' ')}
                </span>
              </>
            )}
            <div className="ml-auto flex items-center gap-2">
              {dirty && <span className="text-[11px] text-warning">unsaved</span>}
              <Button
                size="sm"
                variant="outline"
                disabled={
                  !imageId || !ready || save.isPending || canvasDraft || Boolean(history.gesture)
                }
                onClick={() => save.mutate(false)}
              >
                Save draft
              </Button>
              <Button
                size="sm"
                disabled={
                  !imageId || !ready || save.isPending || canvasDraft || Boolean(history.gesture)
                }
                onClick={() => save.mutate(true)}
              >
                <Check className="h-3.5 w-3.5" /> Save &amp; accept
                <kbd className="rounded border border-current/30 px-1 font-mono text-[10px] opacity-70">
                  a
                </kbd>
              </Button>
            </div>
          </div>

          {detail.isError ? (
            <Card className="p-3">
              <EmptyState
                title="Could not load this image revision"
                hint="Check the image and revision link, or retry the request."
              />
              <Button size="sm" variant="outline" onClick={() => void detail.refetch()}>
                Retry image
              </Button>
            </Card>
          ) : detail.data && ready ? (
            <AnnotationCanvas
              key={context}
              disabled={save.isPending || review.isPending || blocker.status === 'blocked'}
              onEditStart={history.begin}
              onEditEnd={history.end}
              onDraftChange={setCanvasDraft}
              src={imageSrc(detail.data.image.uri)}
              width={detail.data.image.width}
              height={detail.data.image.height}
              classes={classes}
              annotations={annotations}
              tool={tool}
              activeClass={activeClass}
              selectedId={selectedId}
              linkTargets={linking ? annotations.map((annotation) => annotation.id) : []}
              onSelect={onCanvasSelect}
              onChange={(id, geometry) => {
                const annotation = annotations.find((entry) => entry.id === id);
                if (annotation) replace(id, { ...annotation, geometry });
              }}
              onCreate={create}
              invalid={[...invalid]}
              className="h-[calc(100vh-16rem)]"
            />
          ) : (
            <Card className="flex h-[calc(100vh-16rem)] items-center justify-center">
              {imageId && detail.isPending ? (
                <p role="status" className="text-sm text-muted-foreground">
                  Loading image…
                </p>
              ) : (
                <EmptyState title="Pick an image" hint="Or import one to start labelling." />
              )}
            </Card>
          )}

          {problems.length > 0 && (
            <Card className="border-danger/40 bg-danger/5 p-2 text-xs">
              <p className="mb-1 font-medium text-danger">
                The registry refused this drawing. Every problem it found:
              </p>
              <ul className="flex list-inside list-disc flex-col gap-0.5 text-muted-foreground">
                {problems.map((problem) => (
                  <li key={problem}>{problem}</li>
                ))}
              </ul>
            </Card>
          )}
        </div>

        <div className="flex max-h-[calc(100vh-12rem)] flex-col gap-2 overflow-hidden">
          <Card className="overflow-y-auto p-1">
            <ClassPalette
              classes={classes}
              active={activeClass}
              counts={counts}
              onPick={(name) => {
                setActiveClass(name);
                setTool('draw');
              }}
            />
          </Card>
          <Card className="flex-1 overflow-y-auto">
            {selected ? (
              <ShapeInspector
                annotation={selected}
                classes={classes}
                annotations={annotations}
                linking={linking}
                onChange={(next) => replace(selected.id, next)}
                onStartLink={setLinking}
                onClearLink={(name) =>
                  replace(selected.id, {
                    ...selected,
                    links: { ...(selected.links ?? {}), [name]: [] },
                  })
                }
              />
            ) : (
              <ShapeList
                annotations={annotations}
                classes={classes}
                selectedId={selectedId}
                invalid={invalid}
                onSelect={setSelectedId}
                onDelete={(id) =>
                  setDraft(annotations.filter((annotation) => annotation.id !== id))
                }
              />
            )}
          </Card>
          {detail.data?.review === 'accepted' && (
            <Button
              size="sm"
              variant="outline"
              onClick={() => review.mutate('rejected')}
              className="text-xs"
            >
              Mark unusable
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

function ToolButton({
  active,
  onClick,
  hotkey,
  children,
}: {
  active: boolean;
  onClick: () => void;
  hotkey: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        'flex items-center gap-1.5 px-3 py-1.5 text-xs transition-colors',
        active ? 'bg-primary text-primary-foreground' : 'hover:bg-accent',
      )}
    >
      {children}
      <kbd className="rounded border border-current/30 px-1 font-mono text-[10px] opacity-70">
        {hotkey}
      </kbd>
    </button>
  );
}

function FilterSelect({
  value,
  onChange,
  options,
  placeholder,
}: {
  value: string | undefined;
  onChange: (value: string | undefined) => void;
  options: string[];
  placeholder: string;
}) {
  return (
    <select
      aria-label={placeholder === 'review' ? 'Review filter' : 'Split filter'}
      value={value ?? ''}
      onChange={(event) => onChange(event.target.value || undefined)}
      className="flex-1 rounded-md border border-border bg-background px-1.5 py-1 text-[11px]"
    >
      <option value="">{placeholder}</option>
      {options.map((option) => (
        <option key={option} value={option}>
          {option.replace('_', ' ')}
        </option>
      ))}
    </select>
  );
}

/**
 * Upload and register in one step.
 *
 * `group_id` is the field that decides whether the eventual test score means
 * anything, so it is the only required one and it is explained here rather
 * than in a tooltip nobody opens.
 */
function ImageImport({
  project,
  onImported,
}: {
  project: string;
  onImported: (head: ImageHead) => void;
}) {
  const [open, setOpen] = React.useState(false);
  const [file, setFile] = React.useState<File | null>(null);
  const [groupId, setGroupId] = React.useState('');
  const [source, setSource] = React.useState('');
  const [level, setLevel] = React.useState('');
  const [view, setView] = React.useState('');
  const [rights, setRights] = React.useState<UsageRights['kind']>('owned');
  const [license, setLicense] = React.useState('');
  const [error, setError] = React.useState<string | null>(null);

  const upload = useMutation({
    mutationFn: async () => {
      if (!file) throw new Error('pick a file');
      const bytes = new Uint8Array(await file.arrayBuffer());
      const dimensions = await measure(file);
      const stored = await uploadBlob({
        throwOnError: true,
        // The route takes raw bytes; the generated type says `number[]` because
        // OpenAPI has no way to say "a byte stream". `bodySerializer: null`
        // means this is passed through untouched.
        body: bytes as unknown as number[],
        headers: { 'Content-Type': file.type || 'application/octet-stream' },
      });
      const registered = await registerImage({
        throwOnError: true,
        body: {
          project,
          image_id: stored.data.image_id,
          uri: stored.data.uri,
          width: dimensions.width,
          height: dimensions.height,
          group_id: groupId,
          source,
          level: level || null,
          view,
          rights:
            rights === 'owned'
              ? { kind: 'owned', grant: '' }
              : rights === 'unknown'
                ? { kind: 'unknown' }
                : { kind: rights, license },
          metadata: {},
        },
      });
      return registered.data;
    },
    onSuccess: (head) => {
      setOpen(false);
      setFile(null);
      setError(null);
      onImported(head);
    },
    onError: (failure) => setError(rejectionDetails(failure)[0] ?? String(failure)),
  });

  if (!open) {
    return (
      <Button size="sm" variant="outline" onClick={() => setOpen(true)}>
        <Upload className="h-3.5 w-3.5" /> Import
      </Button>
    );
  }

  return (
    <Card className="absolute right-6 z-20 mt-2 flex w-80 flex-col gap-2 p-3 text-xs shadow-lg">
      <input
        type="file"
        accept="image/*"
        onChange={(event) => setFile(event.target.files?.[0] ?? null)}
        className="text-xs"
      />
      <label className="flex flex-col gap-1">
        <span className="font-medium text-muted-foreground">family (group_id) *</span>
        <input
          value={groupId}
          onChange={(event) => setGroupId(event.target.value)}
          placeholder="komancza-dws"
          className="rounded-md border border-border bg-background px-2 py-1"
        />
        <span className="text-[11px] text-muted-foreground">
          One building, however many renderings. The plan, its mirror and the garage variant share
          this, so the split never puts them on opposite sides.
        </span>
      </label>
      <div className="grid grid-cols-2 gap-2">
        <label className="flex flex-col gap-1">
          <span className="font-medium text-muted-foreground">source</span>
          <input
            value={source}
            onChange={(event) => setSource(event.target.value)}
            placeholder="dom-projekt"
            className="rounded-md border border-border bg-background px-2 py-1"
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="font-medium text-muted-foreground">level</span>
          <input
            value={level}
            onChange={(event) => setLevel(event.target.value)}
            placeholder="level, if the corpus has them"
            className="rounded-md border border-border bg-background px-2 py-1"
          />
        </label>
        <label className="flex flex-col gap-1">
          {/* Free text, not a list: an export selects the views it wants by
              name, and the vocabulary is the corpus's. Blank is right for a
              corpus with only one kind of picture, which is most of them. */}
          <span className="font-medium text-muted-foreground">view</span>
          <input
            value={view}
            onChange={(event) => setView(event.target.value)}
            placeholder="only if this corpus mixes kinds"
            className="rounded-md border border-border bg-background px-2 py-1"
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="font-medium text-muted-foreground">rights</span>
          <select
            value={rights}
            onChange={(event) => setRights(event.target.value as UsageRights['kind'])}
            className="rounded-md border border-border bg-background px-2 py-1"
          >
            <option value="owned">owned</option>
            <option value="licensed">licensed</option>
            <option value="research_only">research only</option>
            <option value="unknown">unknown</option>
          </select>
        </label>
      </div>
      {(rights === 'licensed' || rights === 'research_only') && (
        <input
          value={license}
          onChange={(event) => setLicense(event.target.value)}
          placeholder="CC BY-NC 4.0"
          className="rounded-md border border-border bg-background px-2 py-1"
        />
      )}
      <p className="text-[11px] text-muted-foreground">
        A commercial export excludes anything that is not owned or commercially licensed, and names
        it in the manifest.
      </p>
      {error && <p className="text-[11px] text-danger">{error}</p>}
      <div className="flex justify-end gap-2">
        <Button size="sm" variant="ghost" onClick={() => setOpen(false)}>
          Cancel
        </Button>
        <Button
          size="sm"
          disabled={!file || !groupId || upload.isPending}
          onClick={() => upload.mutate()}
        >
          Import
        </Button>
      </div>
    </Card>
  );
}

/**
 * The browser is the only thing here that knows how large a PNG is.
 *
 * The registry needs the dimensions to validate that a shape is inside the
 * image, and decoding an arbitrary image format in Rust to find them would be
 * a decoder in the request path for one integer pair.
 */
async function measure(file: File): Promise<{ width: number; height: number }> {
  const bitmap = await createImageBitmap(file);
  try {
    return { width: bitmap.width, height: bitmap.height };
  } finally {
    bitmap.close();
  }
}

/**
 * The first project, and the vocabulary it starts from.
 *
 * aiwatcher ships none — the label schema is the one thing about an annotation
 * project that is entirely the domain's, and a preset shipped here would be
 * one field's homework imposed on every other. So this offers a *shape* to
 * edit rather than a vocabulary to accept: one filled class, one stroked
 * class, and an ignore class, which is the smallest set that demonstrates what
 * the schema can say.
 *
 * The `ignore` class is not decoration. Whatever a corpus is full of that a
 * model must not be scored on either way — furniture, borders, watermarks —
 * marking it is cheaper than labelling it and far cheaper than the false
 * positives it produces.
 */
const STARTER_CLASSES = [
  {
    name: 'region',
    geometry: 'polygon' as const,
    color: '#2563eb',
    description: 'An enclosed area.',
  },
  {
    name: 'edge',
    geometry: 'polyline' as const,
    color: '#1f2937',
    description: 'A boundary, drawn as a centreline carrying its own width.',
    attributes: [{ name: 'thickness_px', kind: 'number' as const, required: true }],
  },
  {
    name: 'ignore',
    geometry: 'polygon' as const,
    color: '#dc2626',
    description: 'Excluded from every training target and from the loss.',
    ignore: true,
  },
];

function FirstProject({ onCreated }: { onCreated: (name: string) => void }) {
  const [name, setName] = React.useState('corpora/first');

  const create = useMutation({
    mutationFn: async () => {
      const response = await saveProject({
        throwOnError: true,
        body: {
          name,
          description: 'A starting vocabulary, meant to be edited',
          classes: STARTER_CLASSES,
          split_salt: new Date().toISOString().slice(0, 7),
        },
      });
      return response.data;
    },
    onSuccess: (project) => onCreated(project.name),
  });

  return (
    <Card className="mx-auto flex max-w-xl flex-col gap-3 p-6">
      <h2 className="text-sm font-semibold">No annotation project yet</h2>
      <p className="text-xs leading-relaxed text-muted-foreground">
        A project holds the label schema, the split policy and every image drawn against them. The
        classes below are a starting shape rather than a vocabulary: one filled class, one stroked
        class carrying its own width, and an ignore class. Rename them to whatever this corpus is
        actually of &mdash; renaming later is a new schema version and excludes every drawing made
        under the old one, by name.
      </p>
      <input
        value={name}
        onChange={(event) => setName(event.target.value)}
        className="rounded-md border border-border bg-background px-2 py-1.5 text-sm"
      />
      <div className="flex flex-wrap gap-1">
        {STARTER_CLASSES.map((definition) => (
          <Badge key={definition.name} className="gap-1 px-2 py-0.5 text-[11px]">
            <span className="h-2 w-2 rounded-sm" style={{ background: definition.color }} />
            {definition.name}
          </Badge>
        ))}
      </div>
      <Button
        onClick={() => create.mutate()}
        disabled={!name || create.isPending}
        className="self-start"
      >
        Create project
      </Button>
      {create.isError && <p className="text-xs text-danger">{rejectionDetails(create.error)[0]}</p>}
    </Card>
  );
}
