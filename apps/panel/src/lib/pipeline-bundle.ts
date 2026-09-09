import { z } from 'zod';

import type { SavePipelineRequest } from '@/api/generated/types.gen';
import { getNotebook, getNotebookRevision, NotebookError, saveNotebook } from '@/lib/ml-pipeline';
import { orderOf } from '@/lib/pipeline';

export const MAX_BUNDLE_BYTES = 8 * 1024 * 1024;
const bytes = (text: string) => new TextEncoder().encode(text).length;
const name = z.string().regex(/^[a-z][a-z0-9_]{0,63}$/);
const digest = z.string().regex(/^[a-f0-9]{64}$/);
const blockId = z.string().regex(/^[a-z0-9-]{1,40}$/);
const limitedText = (limit: number) =>
  z.string().refine((text) => bytes(text) <= limit, 'Text exceeds byte limit');
const pipelineName = limitedText(160).refine(
  (value) => /^[a-zA-Z0-9][a-zA-Z0-9_.-]*(\/[a-zA-Z0-9][a-zA-Z0-9_.-]*)*$/.test(value),
  'Invalid pipeline or dataset name',
);

/**
 * Whatever a timeout writes as its answer. Not `unknown`: that includes
 * `undefined`, and a key that may be absent is a policy with no answer in it.
 */
const jsonValue = z.union([
  z.string(),
  z.number(),
  z.boolean(),
  z.null(),
  z.array(z.unknown()),
  z.record(z.unknown()),
]);

const spec = z.discriminatedUnion('kind', [
  z
    .object({
      kind: z.literal('source'),
      dataset: z.string().min(1),
      arguments: z.record(z.string()).optional(),
    })
    .strict(),
  z.object({ kind: z.literal('transform'), steps: limitedText(128 * 1024).optional() }).strict(),
  z
    .object({
      kind: z.literal('notebook'),
      notebook: name,
      revision: digest,
      params: z.record(z.unknown()).optional(),
    })
    .strict(),
  z
    .object({
      kind: z.literal('approval'),
      prompt: limitedText(4 * 1024).optional(),
      role: z.string().optional(),
      choices: z.array(limitedText(80)).max(8).optional(),
      timeout_seconds: z.number().int().positive().optional(),
      on_timeout: z
        .discriminatedUnion('on', [
          z.object({ on: z.literal('fail') }).strict(),
          z.object({ on: z.literal('skip') }).strict(),
          // A JSON value rather than `unknown`, and the difference is the one
          // that matters: `unknown` includes `undefined`, which would make the
          // key optional — an `answer` policy that writes nothing.
          z.object({ on: z.literal('answer'), response: jsonValue }).strict(),
        ])
        .optional(),
    })
    .strict(),
  z.object({ kind: z.literal('view'), dataset: pipelineName.nullish() }).strict(),
]);

const bundleSchema = z
  .object({
    format: z.literal('aiwatcher.curation'),
    version: z.literal(1),
    // Requirements are informational; importing never installs packages or runs Python.
    requirements: z.object({ python: z.string(), packages: z.array(z.string()) }).strict(),
    pipeline: z
      .object({
        name: pipelineName,
        description: limitedText(8 * 1024).optional(),
        blocks: z
          .array(
            z
              .object({
                id: blockId,
                title: limitedText(120).optional(),
                position: z
                  .object({ x: z.number().finite().optional(), y: z.number().finite().optional() })
                  .strict()
                  .optional(),
                spec,
              })
              .strict(),
          )
          .min(1)
          .max(24),
        edges: z.array(z.object({ from: blockId, to: blockId }).strict()).max(23),
      })
      .strict(),
    notebooks: z
      .array(z.object({ name, revision: digest, source: limitedText(256 * 1024) }).strict())
      .max(24),
  })
  .strict();

export type PipelineBundle = z.infer<typeof bundleSchema>;
export const BUNDLE_REQUIREMENTS = {
  python: '>=3.14',
  packages: [
    'aiwatcher-ml-pipeline==0.1.0 (includes FlowAI)',
    'marimo>=0.24,<0.25',
    'scikit-learn>=1.8,<1.9',
    'matplotlib>=3.10,<4',
  ],
};

export async function sourceDigest(source: string): Promise<string> {
  const hash = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(source));
  return [...new Uint8Array(hash)].map((byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** Validate the entire file before any source is written to the runtime. */
export async function parseBundle(text: string): Promise<PipelineBundle> {
  if (bytes(text) > MAX_BUNDLE_BYTES) throw new Error('A flow bundle must be at most 8 MiB.');
  const bundle = bundleSchema.parse(JSON.parse(text));
  const { blocks, edges } = bundle.pipeline;
  if (new Set(blocks.map((block) => block.id)).size !== blocks.length) {
    throw new Error('Block IDs must be unique.');
  }
  const chain = orderOf(blocks, edges);
  if (!chain || chain[0]?.spec.kind !== 'source')
    throw new Error('The flow must be one chain starting with a source.');
  let sealed = false;
  for (const [index, block] of chain.entries()) {
    if (
      block.spec.kind === 'source' &&
      Object.values(block.spec.arguments ?? {}).reduce((total, value) => total + bytes(value), 0) >
        32 * 1024
    )
      throw new Error('Source arguments exceed 32 KiB.');
    if (
      block.spec.kind === 'notebook' &&
      bytes(JSON.stringify(block.spec.params ?? {})) > 32 * 1024
    )
      throw new Error('Python parameters exceed 32 KiB.');
    if (index > 0 && block.spec.kind === 'source') throw new Error('A flow has one source.');
    // A data transform is one Flow PHP query, and the query ends at the first
    // block that is not part of it. Everything after that reads rows the query
    // service cannot be handed.
    if (block.spec.kind === 'notebook' || block.spec.kind === 'approval') sealed = true;
    if (sealed && block.spec.kind === 'transform')
      throw new Error('Data transforms must precede Python and approval blocks.');
    if (block.spec.kind === 'view' && index !== chain.length - 1)
      throw new Error('Publish must end the flow.');
  }
  const sources = new Map<string, PipelineBundle['notebooks'][number]>();
  for (const notebook of bundle.notebooks) {
    const key = `${notebook.name}@${notebook.revision}`;
    if (sources.has(key)) throw new Error(`Duplicate source: ${key}`);
    if ((await sourceDigest(notebook.source)) !== notebook.revision)
      throw new Error(`Source checksum mismatch: ${notebook.name}`);
    sources.set(key, notebook);
  }
  const used = new Set<string>();
  for (const block of blocks) {
    if (block.spec.kind !== 'notebook') continue;
    const key = `${block.spec.notebook}@${block.spec.revision}`;
    if (!sources.has(key)) throw new Error(`Missing source: ${key}`);
    used.add(key);
  }
  if (used.size !== sources.size) throw new Error('The bundle contains unreferenced Python code.');
  return bundle;
}

/** Pinned revisions are exported exactly; missing history never falls back to head. */
export async function exportBundle(pipeline: SavePipelineRequest): Promise<PipelineBundle> {
  const blocks = structuredClone(pipeline.blocks);
  const notebooks = new Map<string, PipelineBundle['notebooks'][number]>();
  for (const block of blocks) {
    if (block.spec.kind !== 'notebook') continue;
    const loaded = block.spec.revision
      ? await getNotebookRevision(block.spec.notebook, block.spec.revision)
      : await getNotebook(block.spec.notebook);
    block.spec.revision = loaded.revision;
    notebooks.set(`${loaded.name}@${loaded.revision}`, {
      name: loaded.name,
      revision: loaded.revision,
      source: loaded.source,
    });
  }
  return parseBundle(
    JSON.stringify({
      format: 'aiwatcher.curation',
      version: 1,
      requirements: BUNDLE_REQUIREMENTS,
      pipeline: { ...pipeline, blocks, edges: pipeline.edges ?? [] },
      notebooks: [...notebooks.values()],
    }),
  );
}

/** Install under content-derived names so another flow's editable source survives. */
export async function importBundle(text: string): Promise<SavePipelineRequest> {
  const bundle = await parseBundle(text);
  const names = new Map<string, string>();
  for (const notebook of bundle.notebooks) {
    const key = `${notebook.name}@${notebook.revision}`;
    const importedName = `${notebook.name.replace(/_[a-f0-9]{20}$/, '').slice(0, 43)}_${notebook.revision.slice(0, 20)}`;
    let existing;
    try {
      existing = await getNotebook(importedName);
    } catch (error) {
      if (!(error instanceof NotebookError) || error.status !== 404) throw error;
    }
    if (existing && existing.revision !== notebook.revision) {
      throw new Error(
        `Imported source ${importedName} has local edits. Rename that notebook before importing again.`,
      );
    }
    if (!existing) {
      const saved = await saveNotebook(importedName, notebook.source);
      if (saved.revision !== notebook.revision)
        throw new Error(`The runtime changed source: ${notebook.name}`);
    }
    names.set(key, importedName);
  }
  return {
    ...bundle.pipeline,
    blocks: bundle.pipeline.blocks.map((block) =>
      block.spec.kind === 'notebook'
        ? {
            ...block,
            spec: {
              ...block.spec,
              notebook: names.get(`${block.spec.notebook}@${block.spec.revision}`)!,
            },
          }
        : block,
    ),
  };
}
