import { describe, expect, it } from 'vitest';

import { allCommands, fuzzyScore, search } from '@/app/commands';
import { SECTIONS } from '@/app/navigation';

import { searchSchema as annotationsExports } from '@/features/annotations/screens/exports/search';
import { searchSchema as annotationsImports } from '@/features/annotations/screens/imports/search';
import { searchSchema as annotationsLabel } from '@/features/annotations/screens/label/search';
import { searchSchema as annotationsSources } from '@/features/annotations/screens/sources/search';
import { searchSchema as conversationsCorpora } from '@/features/conversations/screens/corpora/search';
import { searchSchema as conversationsReview } from '@/features/conversations/screens/review/search';
import { searchSchema as curationPipeline } from '@/features/data-curation/screens/pipeline/search';
import { searchSchema as curationRecipe } from '@/features/data-curation/screens/recipe/search';
import { searchSchema as datasets } from '@/features/datasets/screens/overview/search';
import { searchSchema as evaluation } from '@/features/evaluation/screens/overview/search';
import { searchSchema as experiments } from '@/features/experiments/screens/overview/search';
import { searchSchema as observabilityExplore } from '@/features/observability/screens/explore/search';
import { searchSchema as observabilityLive } from '@/features/observability/screens/live/search';
import { searchSchema as observabilityMetrics } from '@/features/observability/screens/metrics/search';
import { searchSchema as observabilityQuery } from '@/features/observability/screens/query/search';
import { searchSchema as observabilityRuns } from '@/features/observability/screens/runs/search';
import { searchSchema as prompts } from '@/features/prompts/screens/list/search';
import { searchSchema as trainingModels } from '@/features/training/screens/models/search';
import { searchSchema as trainingRuns } from '@/features/training/screens/runs/search';
import { searchSchema as workflows } from '@/features/workflows/screens/overview/search';

/**
 * Each route's own URL contract, by path.
 *
 * This is the only place the palette and the routes are held against each
 * other, and it is a test rather than production code on purpose: a command is
 * `{to, search}` and the route decides what that search may say, so the way
 * they drift is a filter a page has no parameter for. That is invisible at
 * runtime — zod strips what it does not know, so the command navigates, the
 * page renders, and the filter silently does nothing.
 *
 * It has already caught two: `status` on Live, which the view says out loud it
 * cannot honour because status is assembled from several events (ADR_0003),
 * and `status` on Explore, which has no such parameter at all.
 */
const SCHEMAS: Record<string, { parse: (value: unknown) => unknown }> = {
  '/annotations/exports': annotationsExports,
  '/annotations/imports': annotationsImports,
  '/annotations/label': annotationsLabel,
  '/annotations/sources': annotationsSources,
  '/conversations/corpora': conversationsCorpora,
  '/conversations/review': conversationsReview,
  '/data-curation/pipeline': curationPipeline,
  '/data-curation/recipe': curationRecipe,
  '/datasets': datasets,
  '/evaluation': evaluation,
  '/experiments': experiments,
  '/observability/explore': observabilityExplore,
  '/observability/live': observabilityLive,
  '/observability/metrics': observabilityMetrics,
  '/observability/query': observabilityQuery,
  '/observability/runs': observabilityRuns,
  '/prompts': prompts,
  '/training/models': trainingModels,
  '/training/runs': trainingRuns,
  '/workflows': workflows,
};

/**
 * What these are for: a palette is only useful if the thing somebody half
 * remembers is the thing that comes first. Everything below is a phrase
 * somebody would actually type.
 */

describe('matching a command', () => {
  const commands = allCommands();

  function first(query: string) {
    return search(commands, query)[0];
  }

  it('finds the live trace view from the words in the request', () => {
    expect(first('show traces for live agents')?.id).toBe('traces:live');
    expect(first('live')?.id).toBe('traces:live');
  });

  it('reaches it from an initialism, the way a shell completes', () => {
    // The property that makes this feel like a CLI rather than a menu: the
    // characters in order, not a substring.
    expect(first('stfla')?.id).toBe('traces:live');
  });

  it('lands "create a new dataset" on the view where one is actually authored', () => {
    // There is no create-dataset dialog: a version is published by a
    // curation's view block. The command says so rather than implying one.
    const command = first('create a new dataset');
    expect(command?.id).toBe('dataset:new');
    expect(command?.to).toBe('/data-curation/recipe');
    expect(command?.hint).toMatch(/published by a curation/);
  });

  it('carries the filter, not just the page', () => {
    expect(first('runs that failed')?.search).toEqual({ status: 'failed' });
    expect(first('training runs in flight')?.search).toEqual({ status: 'running' });
    expect(first('compare runs by model')?.search).toEqual({ by: 'model' });
  });

  it('sets no filter on the view that is already the thing asked for', () => {
    // Live *is* the log as it arrives. A `status: running` beside it read like
    // a narrowing and was none — the view says so on the page, because status
    // is assembled from several events and no single one carries it.
    const live = first('show traces for live agents');
    expect(live?.to).toBe('/observability/live');
    expect(live?.search).toBeUndefined();
  });

  it('finds a command by a word that is nowhere in its label', () => {
    // "tail" and "p95" are what somebody types; neither is in the name.
    expect(first('tail')?.id).toBe('traces:live');
    expect(first('p95')?.id).toBe('traces:metrics');
    expect(first('kaggle')?.id).toBe('dataset:discover');
  });

  it('ranks the command being named above one that merely lists the word', () => {
    // The failure this is against: a command with a long keyword list beating
    // the one whose label is being typed.
    const ranked = search(commands, 'live');
    expect(ranked[0]?.id).toBe('traces:live');
  });

  it('answers an empty query with everything, in a stable order', () => {
    expect(search(commands, '')).toHaveLength(commands.length);
    expect(search(commands, '   ')).toHaveLength(commands.length);
  });

  it('matches nothing rather than everything when nothing matches', () => {
    expect(search(commands, 'zzzzqqq')).toEqual([]);
  });
});

describe('the registry itself', () => {
  const commands = allCommands();

  it('has one command per page the navigation describes', () => {
    // Derived rather than listed again: the failure this catches is an area
    // added to the sidebar that the palette cannot reach.
    const pages = SECTIONS.flatMap((section) =>
      section.areas.flatMap((area) =>
        area.views.length > 0 ? area.views.map((v) => v.to) : [area.to],
      ),
    );
    for (const page of pages) {
      expect(commands.some((command) => command.id === `go:${page}`)).toBe(true);
    }
  });

  it('gives every command a unique id', () => {
    const ids = commands.map((command) => command.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('points every command at a path the navigation knows', () => {
    // A command is a link, and a link to a route that does not exist is the
    // one failure a palette makes look like a broken page.
    const known = new Set(
      SECTIONS.flatMap((section) =>
        section.areas.flatMap((area) => [area.to, ...area.views.map((view) => view.to)]),
      ),
    );
    for (const command of commands) {
      expect(known.has(command.to), `${command.id} → ${command.to}`).toBe(true);
    }
  });
});

describe('every command against the route it targets', () => {
  const commands = allCommands();

  it('covers every page the navigation describes, so nothing is unchecked', () => {
    const pages = SECTIONS.flatMap((section) =>
      section.areas.flatMap((area) =>
        area.views.length > 0 ? area.views.map((view) => view.to) : [area.to],
      ),
    );
    for (const page of pages) {
      expect(Object.keys(SCHEMAS), `${page} has no schema in this test`).toContain(page);
    }
  });

  it('sets only parameters the route actually understands', () => {
    // The failure this is against is silent: zod strips an unknown key, so the
    // command navigates, the page renders, and the filter does nothing.
    for (const command of commands) {
      const given = command.search;
      if (!given) continue;
      const schema = SCHEMAS[command.to];
      expect(schema, `${command.id} points at ${command.to}, which has no schema`).toBeDefined();

      const parsed = schema!.parse(given) as Record<string, unknown>;
      for (const key of Object.keys(given)) {
        expect(
          Object.keys(parsed),
          `${command.id} sets ${key}, which ${command.to} drops`,
        ).toContain(key);
        expect(parsed[key], `${command.id}'s ${key} is not what the route reads back`).toEqual(
          given[key],
        );
      }
    }
  });
});

describe('the score', () => {
  it('is null for a miss and a number for a hit', () => {
    expect(fuzzyScore('Show traces', 'xyz')).toBeNull();
    expect(fuzzyScore('Show traces', 'show')).not.toBeNull();
  });

  it('prefers a match at a word start over one buried in a word', () => {
    const atStart = fuzzyScore('Show traces for live agents', 'live') ?? -Infinity;
    const buried = fuzzyScore('Delivered something', 'live') ?? -Infinity;
    expect(atStart).toBeGreaterThan(buried);
  });
});
