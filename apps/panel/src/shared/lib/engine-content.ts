import * as datafusion from '@/shared/lib/datafusion';
import * as duckdb from '@/shared/lib/duckdb';
import * as flow from '@/shared/lib/flow';
import type { QueryExample } from '@/shared/lib/flow';
import type { QueryEngineName } from '@/shared/lib/query';

/**
 * What the panel ships in each engine's language, chosen by the engine the
 * deployment runs (AW-3).
 *
 * An engine absent here would have no content in the build, and a screen says
 * so rather than offering another language's text to run, which the engine
 * would refuse as a syntax error somebody reads as their own.
 */
export interface EngineContent {
  starterQuery: string;
  starterCuration: string;
  examples: QueryExample[];
  transformations: ReadonlyArray<readonly [string, string, string]>;
  newTransform: string;
  transformHelp: string;
}

const CONTENT: Partial<Record<QueryEngineName, EngineContent>> = {
  flow: {
    starterQuery: flow.STARTER_QUERY,
    starterCuration: flow.STARTER_CURATION,
    examples: flow.QUERY_EXAMPLES,
    transformations: flow.TRANSFORMATIONS,
    newTransform: flow.NEW_TRANSFORM,
    transformHelp: flow.TRANSFORM_HELP,
  },
  datafusion: {
    starterQuery: datafusion.STARTER_QUERY,
    starterCuration: datafusion.STARTER_CURATION,
    examples: datafusion.QUERY_EXAMPLES,
    transformations: datafusion.TRANSFORMATIONS,
    newTransform: datafusion.NEW_TRANSFORM,
    transformHelp: datafusion.TRANSFORM_HELP,
  },
  duckdb: {
    starterQuery: duckdb.STARTER_QUERY,
    starterCuration: duckdb.STARTER_CURATION,
    examples: duckdb.QUERY_EXAMPLES,
    transformations: duckdb.TRANSFORMATIONS,
    newTransform: duckdb.NEW_TRANSFORM,
    transformHelp: duckdb.TRANSFORM_HELP,
  },
};

/** The content for an engine; Flow's while the deployed engine is not known yet. */
export function contentFor(engine: QueryEngineName | null | undefined): EngineContent | undefined {
  return CONTENT[engine ?? 'flow'];
}
