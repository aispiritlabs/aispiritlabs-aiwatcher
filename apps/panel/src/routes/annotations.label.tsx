import * as React from 'react';
import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, MousePointer2, PenLine, Upload } from 'lucide-react';
import { z } from 'zod';

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
  LabelClass,
  ReviewState,
  Split,
  UsageRights,
} from '@/api/generated/types.gen';
import { AnnotationCanvas, type Tool } from '@/components/annotation-canvas';
import { ClassPalette, ShapeInspector, ShapeList } from '@/components/annotation-inspector';
import { RegistryDisabled, isRegistryDisabled } from '@/components/registry-disabled';
import { Badge, Button, Card, EmptyState, Spinner } from '@/components/ui/primitives';
import {
  REVIEW_TONES,
  SPLIT_LABELS,
  classOf,
  defaultAttributes,
  nextId,
  rejectionDetails,
  sameAnnotations,
} from '@/lib/annotations';
import { cn } from '@/lib/utils';

/**
 * The labelling workspace.
 *
 * Three columns, in the order attention moves: what to label, the plan, what
 * the shape says. The middle column is the only one that matters and gets the
 * space; the other two are lists.
 *
 * Two decisions here are worth stating, because both look like omissions.
 *
 * The draft lives in component state and is *not* written back on every
 * change. A revision is content-addressed and immutable, so autosaving every
 * vertex drag would mint a revision per mouse move. Save is explicit, and
 * saving the same drawing twice is one revision anyway.
 *
 * Validation is not repeated here. The registry refuses a bad drawing with one
 * line per problem, and the panel renders exactly those lines. A second
 * implementation in TypeScript would drift from the first, and the day it does
 * is the day somebody trusts the wrong one.
 */

const searchSchema = z.object({
  project: z.string().optional(),
  image: z.string().optional(),
  review: z.enum(['draft', 'in_review', 'accepted', 'rejected']).optional(),
  split: z.enum(['train', 'validation', 'test']).optional(),
  q: z.string().optional(),
});

export const Route = createFileRoute('/annotations/label')({
  validateSearch: searchSchema,
  component: LabelPage,
});

/** `aiwatcher://blob/<sha>` → the route that serves those bytes. */
function imageSrc(uri: string): string {
  const prefix = 'aiwatcher://blob/';
  const base = import.meta.env.VITE_API_BASE_URL ?? '';
  return uri.startsWith(prefix)
    ? `${base}/api/v1/annotation-blobs/${uri.slice(prefix.length)}`
    : uri;
}

function LabelPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
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
      onSearch={(next) => navigate({ search: (previous) => ({ ...previous, ...next }) })}
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
  onSearch: (next: Partial<z.infer<typeof searchSchema>>) => void;
  onInvalidate: () => void;
}) {
  const queryClient = useQueryClient();
  const [tool, setTool] = React.useState<Tool>('select');
  const [activeClass, setActiveClass] = React.useState<string>('');
  const [draft, setDraft] = React.useState<Annotation[] | null>(null);
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [linking, setLinking] = React.useState<string | null>(null);
  const [problems, setProblems] = React.useState<string[]>([]);

  const project = useQuery({
    queryKey: ['annotation-project', projectName],
    queryFn: async () => {
      const response = await getProject({ throwOnError: true, query: { name: projectName } });
      return response.data;
    },
  });

  const images = useQuery({
    queryKey: ['annotation-images', projectName, search.review, search.split, search.q],
    queryFn: async () => {
      const response = await listImages({
        throwOnError: true,
        query: {
          project: projectName,
          review: search.review,
          split: search.split,
          search: search.q || undefined,
          limit: 200,
        },
      });
      return response.data;
    },
  });

  const imageId = search.image ?? images.data?.images[0]?.image.image_id;

  const detail = useQuery({
    queryKey: ['annotation-image', projectName, imageId],
    enabled: Boolean(imageId),
    queryFn: async () => {
      const response = await getImage({
        throwOnError: true,
        query: { project: projectName, image_id: imageId ?? '' },
      });
      return response.data;
    },
  });

  const classes: LabelClass[] = project.data?.schema.classes ?? [];

  // The draft resets when a different image is opened, and starts from that
  // image's accepted revision. Anything unsaved is lost, which is why saving
  // is one keystroke away and the button says so.
  React.useEffect(() => {
    setDraft(detail.data?.revision?.annotations ?? []);
    setSelectedId(null);
    setLinking(null);
    setProblems([]);
  }, [detail.data?.revision?.revision, imageId]);

  React.useEffect(() => {
    if (!activeClass && classes[0]) setActiveClass(classes[0].name);
  }, [activeClass, classes]);

  const annotations = draft ?? [];
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
    onSuccess: (saved) => {
      setProblems([]);
      queryClient.setQueryData(['annotation-image', projectName, imageId], {
        ...detail.data,
        ...saved.head,
        revision: saved.revision,
      });
      void queryClient.invalidateQueries({ queryKey: ['annotation-image', projectName, imageId] });
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

  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
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
      } else if ((event.key === 'Delete' || event.key === 'x') && selectedId) {
        setDraft(annotations.filter((annotation) => annotation.id !== selectedId));
        setSelectedId(null);
      } else if (event.key === 's' && (event.metaKey || event.ctrlKey)) {
        event.preventDefault();
        save.mutate(false);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [annotations, classes, save, selectedId]);

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

  const dirty = draft !== null && !sameAnnotations(draft, detail.data?.revision?.annotations ?? []);

  return (
    <div className="flex flex-col gap-3">
      <header className="flex flex-wrap items-center gap-2">
        <select
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
        <Card className="flex max-h-[calc(100vh-12rem)] flex-col overflow-hidden">
          <div className="flex flex-col gap-2 border-b border-border p-2">
            <input
              defaultValue={search.q ?? ''}
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
          <ul className="flex-1 overflow-y-auto p-1">
            {(images.data?.images ?? []).map((head) => (
              <li key={head.image.image_id}>
                <button
                  type="button"
                  onClick={() => onSearch({ image: head.image.image_id })}
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
              </li>
            ))}
            {images.data && images.data.images.length === 0 && (
              <EmptyState
                title="No images yet"
                hint="Add one with Import, or register a URL through the API."
              />
            )}
          </ul>
        </Card>

        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <div className="flex overflow-hidden rounded-md border border-border">
              <ToolButton active={tool === 'select'} onClick={() => setTool('select')} hotkey="v">
                <MousePointer2 className="h-3.5 w-3.5" /> Select
              </ToolButton>
              <ToolButton active={tool === 'draw'} onClick={() => setTool('draw')} hotkey="d">
                <PenLine className="h-3.5 w-3.5" /> Draw
              </ToolButton>
            </div>
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
                disabled={!imageId || save.isPending}
                onClick={() => save.mutate(false)}
              >
                Save draft
              </Button>
              <Button
                size="sm"
                disabled={!imageId || save.isPending}
                onClick={() => save.mutate(true)}
              >
                <Check className="h-3.5 w-3.5" /> Save &amp; accept
              </Button>
            </div>
          </div>

          {detail.data ? (
            <AnnotationCanvas
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
              <EmptyState title="Pick an image" hint="Or import one to start labelling." />
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
