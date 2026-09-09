import { readFileSync } from 'node:fs';
import { webcrypto } from 'node:crypto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { getNotebook, getNotebookRevision, NotebookError, saveNotebook } from './ml-pipeline';
import {
  exportBundle,
  importBundle,
  parseBundle,
  sourceDigest,
  type PipelineBundle,
} from './pipeline-bundle';
import { runPipeline } from './pipeline';
import { runNotebook } from './ml-pipeline';
import { simulateQuery } from './flow';

vi.mock('./ml-pipeline', async (original) => ({
  ...(await original<typeof import('./ml-pipeline')>()),
  getNotebook: vi.fn(),
  getNotebookRevision: vi.fn(),
  saveNotebook: vi.fn(),
  runNotebook: vi.fn(),
}));
vi.mock('./flow', async (original) => ({
  ...(await original<typeof import('./flow')>()),
  simulateQuery: vi.fn(),
}));

const fixture = readFileSync('../../examples/titanic/titanic.flow.json', 'utf8');
const source = 'import marimo\napp = marimo.App()\n# café\n';

async function smallBundle(): Promise<PipelineBundle> {
  const revision = await sourceDigest(source);
  return {
    format: 'aiwatcher.curation',
    version: 1,
    requirements: { python: '>=3.14', packages: [] },
    pipeline: {
      name: 'curation/test',
      description: 'Round trip',
      blocks: [
        { id: 'source', title: 'Source', spec: { kind: 'source', dataset: 'runs' } },
        {
          id: 'missing',
          title: 'Missing values',
          position: { x: 120, y: 80 },
          spec: { kind: 'notebook', notebook: 'missing', revision, params: { columns: ['Age'] } },
        },
      ],
      edges: [{ from: 'source', to: 'missing' }],
    },
    notebooks: [{ name: 'missing', source, revision }],
  };
}

beforeEach(() => {
  vi.stubGlobal('crypto', webcrypto);
  vi.mocked(getNotebook).mockRejectedValue(new NotebookError('Missing', '', '', 404));
});
afterEach(() => {
  vi.resetAllMocks();
  vi.unstubAllGlobals();
});

describe('portable curation flows', () => {
  it('preserves native PHP preparation in the hybrid Titanic bundle', async () => {
    const bundle = await parseBundle(
      readFileSync('../../examples/titanic/titanic-php.flow.json', 'utf8'),
    );
    expect(bundle.notebooks).toHaveLength(2);
    const encoder = bundle.pipeline.blocks.find((block) => block.id === 'one-hot');
    expect(encoder?.spec.kind).toBe('transform');
    expect(JSON.stringify(encoder?.spec)).toContain('oneHotEncode');
    expect(bundle.pipeline.edges).toContainEqual({ from: 'label', to: 'visualization' });
    vi.mocked(saveNotebook).mockImplementation(async (name, source) => ({
      name,
      source,
      revision: await sourceDigest(source),
      title: name,
      size: source.length,
      modified_at: '',
      app_url: '',
    }));
    const imported = await importBundle(JSON.stringify(bundle));
    vi.mocked(getNotebookRevision).mockImplementation(async (name, revision) => {
      const source = bundle.notebooks.find((entry) => entry.revision === revision)!.source;
      return {
        name,
        source,
        revision,
        title: name,
        size: source.length,
        modified_at: '',
        app_url: '',
      };
    });
    const exported = await exportBundle(imported);
    const reimported = await importBundle(JSON.stringify(exported));
    expect(reimported).toEqual(imported);
    expect(exported.pipeline.blocks.filter((block) => block.spec.kind === 'transform')).toEqual(
      bundle.pipeline.blocks.filter((block) => block.spec.kind === 'transform'),
    );
    expect(exported.notebooks.map((entry) => entry.source)).toEqual(
      bundle.notebooks.map((entry) => entry.source),
    );
  });
  it('validates the real Titanic artifact including every embedded source', async () => {
    const bundle = await parseBundle(fixture);
    expect(bundle.notebooks).toHaveLength(7);
    expect(bundle.pipeline.blocks.map((block) => block.title)).toContain('Visualization');
  });

  it('imports into isolated names and exports exactly the pinned sources, parameters and layout', async () => {
    const original = await smallBundle();
    const entry = original.notebooks[0]!;
    const summary = {
      ...entry,
      size: source.length,
      title: 'Missing',
      modified_at: '',
      app_url: '',
    };
    vi.mocked(saveNotebook).mockImplementation(async (name, source) => ({
      ...summary,
      name,
      source,
    }));
    const imported = await importBundle(JSON.stringify(original));
    const block = imported.blocks[1]!;
    expect(block.title).toBe('Missing values');
    expect(block.position).toEqual({ x: 120, y: 80 });
    if (block.spec.kind !== 'notebook') throw new Error('Expected Python');
    expect(block.spec.notebook).toBe(`missing_${entry.revision.slice(0, 20)}`);
    expect(block.spec.params).toEqual({ columns: ['Age'] });
    vi.mocked(getNotebookRevision).mockResolvedValue({ ...summary, name: block.spec.notebook });
    const exported = await exportBundle(imported);
    expect(getNotebookRevision).toHaveBeenCalledWith(block.spec.notebook, entry.revision);
    expect(exported.notebooks[0]?.source).toBe(source);
    expect(exported.pipeline).toEqual(imported);
  });

  it.each([
    'checksum',
    'missing source',
    'duplicate id',
    'dangling edge',
    'cycle',
    'version',
    'extra source',
    'unknown kind',
    'invalid name',
  ])('rejects %s before writing any Python', async (problem) => {
    const bundle = await smallBundle();
    if (problem === 'checksum') bundle.notebooks[0]!.source += '# changed';
    if (problem === 'missing source') bundle.notebooks = [];
    if (problem === 'duplicate id') bundle.pipeline.blocks[1]!.id = 'source';
    if (problem === 'dangling edge') bundle.pipeline.edges[0]!.to = 'absent';
    if (problem === 'cycle') bundle.pipeline.edges.push({ from: 'missing', to: 'source' });
    if (problem === 'extra source')
      bundle.notebooks.push({ ...bundle.notebooks[0]!, name: 'extra' });
    if (problem === 'invalid name') bundle.notebooks[0]!.name = '../outside';
    let text = JSON.stringify(bundle);
    if (problem === 'version') text = text.replace('"version":1', '"version":99');
    if (problem === 'unknown kind') text = text.replace('"kind":"notebook"', '"kind":"unknown"');
    await expect(importBundle(text)).rejects.toThrow();
    expect(saveNotebook).not.toHaveBeenCalled();
  });

  it('carries an approval gate through an export and back', async () => {
    // A .flow.json is what somebody hands to a colleague, so a chain that
    // saves has to be one the importer accepts. A gate the schema did not know
    // about would export perfectly well and refuse to come back.
    const bundle = await smallBundle();
    bundle.pipeline.blocks.push({
      id: 'sign-off',
      title: 'Approval',
      spec: {
        kind: 'approval',
        prompt: 'Publish these rows?',
        role: 'editor',
        choices: ['approve', 'reject'],
      },
    });
    bundle.pipeline.edges.push({ from: 'missing', to: 'sign-off' });

    const parsed = await parseBundle(JSON.stringify(bundle));

    expect(parsed.pipeline.blocks.at(-1)?.spec).toEqual({
      kind: 'approval',
      prompt: 'Publish these rows?',
      role: 'editor',
      choices: ['approve', 'reject'],
    });
  });

  it('refuses a data transform behind an approval, as the registry does', async () => {
    const bundle = await smallBundle();
    bundle.pipeline.blocks = [
      { id: 'source', title: 'Source', spec: { kind: 'source', dataset: 'runs' } },
      {
        id: 'sign-off',
        title: 'Approval',
        spec: { kind: 'approval', prompt: 'Go on?', role: 'editor', choices: [] },
      },
      { id: 'shape', title: 'Shape', spec: { kind: 'transform', steps: '->limit(10)' } },
    ];
    bundle.pipeline.edges = [
      { from: 'source', to: 'sign-off' },
      { from: 'sign-off', to: 'shape' },
    ];
    bundle.notebooks = [];

    await expect(parseBundle(JSON.stringify(bundle))).rejects.toThrow(/must precede/);
  });

  it('refuses to overwrite locally edited imported sources', async () => {
    vi.mocked(getNotebook).mockResolvedValue({
      name: 'missing',
      source: 'edited',
      revision: '0'.repeat(64),
      size: 6,
      title: '',
      modified_at: '',
      app_url: '',
    });
    await expect(importBundle(JSON.stringify(await smallBundle()))).rejects.toThrow('local edits');
    expect(saveNotebook).not.toHaveBeenCalled();
  });

  it('never exports the head when the pinned revision is unavailable', async () => {
    vi.mocked(getNotebookRevision).mockRejectedValue(new Error('History missing'));
    await expect(exportBundle((await smallBundle()).pipeline)).rejects.toThrow('History missing');
    expect(getNotebook).not.toHaveBeenCalled();
  });

  it('passes the imported revision through the browser preview path', async () => {
    const bundle = await smallBundle();
    const rows = [{ Age: 20 }];
    vi.mocked(simulateQuery).mockResolvedValue({
      columns: ['Age'],
      rows,
      row_count: 1,
      truncated: false,
      truncate_cells: false,
      dataset: null,
      grain: null,
      source: 'test',
      took_ms: 1,
    });
    vi.mocked(runNotebook).mockResolvedValue({
      notebook: 'missing',
      revision: bundle.notebooks[0]!.revision,
      columns: ['Age'],
      rows,
      row_count: 1,
      truncated: false,
      stdout: '',
      took_ms: 1,
      app_url: '',
    });
    await runPipeline({ chain: bundle.pipeline.blocks, mode: 'preview' });
    expect(runNotebook).toHaveBeenCalledWith(
      'missing',
      rows,
      { columns: ['Age'] },
      bundle.notebooks[0]!.revision,
    );
  });
});
