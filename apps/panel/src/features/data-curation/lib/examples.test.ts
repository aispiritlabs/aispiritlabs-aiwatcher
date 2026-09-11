import { describe, expect, it } from 'vitest';

import * as datafusion from '@/shared/lib/datafusion';
import * as duckdb from '@/shared/lib/duckdb';
import { QUERY_EXAMPLES } from '@/shared/lib/flow';
import { compileFlow, compilePython, orderOf } from '@/features/data-curation/lib/pipeline';
import { readFileSync } from 'node:fs';
import type { SavePipelineRequest } from '@/api/generated/types.gen';
const examples: SavePipelineRequest[] = JSON.parse(
  readFileSync('../../examples/seed.json', 'utf8'),
).pipelines;

/**
 * The examples are shipped code, so they are held to what a chain has to be.
 *
 * Not a second copy of the registry's rules — those are in Rust and a refusal
 * carries every problem at once. What this asks is the traversal's own
 * question, of the three chains this build offers a person to press Preview
 * on: an example that no longer forms one chain would be a button that loads a
 * canvas and then refuses to run, discovered by whoever clicked it.
 */
describe('the shipped curation examples', () => {
  it.each(examples.map((example) => [example.name, example] as const))(
    '%s is one chain, from a source to a view',
    (_title, example) => {
      const chain = orderOf(example.blocks, example.edges ?? []);

      expect(chain).not.toBeNull();
      expect(chain?.[0]?.spec.kind).toBe('source');
      expect(chain?.[chain.length - 1]?.spec.kind).toBe('view');
    },
  );

  it('compiles a Titanic chain into one query the Flow service is given whole', () => {
    const example = examples.find((candidate) => candidate.name === 'curation/titanic-features');
    const chain = orderOf(example?.blocks ?? [], example?.edges ?? []);
    const script = compileFlow(chain ?? []);

    // The read carries the corpus the block names, and every transform line
    // arrives indented into the same pipeline — three boxes, one query.
    expect(script).toContain("->read(hub_rows, dataset: 'phihung/titanic', split: 'train'");
    expect(script).toContain("->withEntry('sex_code', when(ref('sex')->same(lit('female'))");
    expect(script.trimEnd().endsWith('->run();')).toBe(true);
  });

  it('escapes the title pattern as a regular expression rather than as a string', () => {
    // `'\s'` in TypeScript is the letter s, so a pattern written without the
    // second backslash reaches Flow as `/^[^,]*,s*([^.]+)../` — which still
    // matches things, quietly and wrongly.
    const steps = examples
      .flatMap((example) =>
        example.blocks.flatMap((block) =>
          block.spec.kind === 'transform' ? [block.spec.steps] : [],
        ),
      )
      .join('\n');

    expect(steps).toContain("regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*$/')");
  });
});

/**
 * The single-script examples, held to the one thing this file can check.
 *
 * Whether a query is *valid* is the Flow service's answer and nobody else's —
 * `checkQuery` asks it, and a second opinion in TypeScript would be a parser
 * this repository would then have to keep in step with a whitelist in PHP.
 * What is checkable here is that each one is a whole script somebody can press
 * Test on, and that the pattern survived being written in a language where
 * `\s` is the letter s.
 */
describe('the shipped query examples', () => {
  it.each(QUERY_EXAMPLES.map((example) => [example.name, example] as const))(
    '%s is a complete script with a name and a dataset to save it under',
    (_title, example) => {
      expect(example.query.startsWith('data_frame()')).toBe(true);
      expect(example.query.trimEnd().endsWith('->run();')).toBe(true);
      expect(example.name).not.toBe('');
      expect(example.dataset).not.toBe('');
    },
  );

  it('reads the Titanic corpus through the hub rows dataset', () => {
    const titanic = QUERY_EXAMPLES.filter((example) => example.name.startsWith('titanic/'));

    expect(titanic).toHaveLength(4);
    for (const example of titanic) {
      expect(example.query).toContain("->read(hub_rows, dataset: 'phihung/titanic'");
    }
    // And every one that recodes a status extracts the title the same way.
    for (const example of titanic.filter((candidate) => candidate.query.includes("'status'"))) {
      expect(example.query).toContain("regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*$/')");
    }
  });

  it('does the whole feature engineering in one query, where it used to need two engines', () => {
    // A window puts a group's median beside every row, `plus` is arithmetic
    // Flow always had, and `coalesce` does the fill — none of which this
    // language could say while its vocabulary was a hand-written list.
    const features = QUERY_EXAMPLES.find((example) => example.name === 'titanic/features');

    expect(features?.query).toContain(
      "median(ref('age'))->over(window()->partitionBy(ref('status')))",
    );
    expect(features?.query).toContain("ref('sib_sp')->plus(ref('parch'))->plus(lit(1))");
    expect(features?.query).toContain("coalesce(ref('age'), ref('typical_age'))");
    // And the guess stays distinguishable from the measurement.
    expect(features?.query).toContain("->withEntry('age_imputed', ref('age')->isNull())");
  });

  it('asks for a distribution with the aggregations this service adds to Flow', () => {
    // `median`, `stddev` and `percentile` are `services/query/flow`'s own — Flow
    // ships no distribution aggregation at all — so an example naming them is
    // also the thing that would break if that whitelist entry were dropped.
    const spread = QUERY_EXAMPLES.find((example) => example.name === 'titanic/age-and-fare');

    expect(spread?.query).toContain("median(ref('age'))");
    expect(spread?.query).toContain("stddev(ref('age')->as('age_spread'))");
    expect(spread?.query).toContain("percentile(ref('fare')->as('fare_p90'), 90)");
  });

  it('has a DataFusion counterpart for every example, under the same name', () => {
    // A DataFusion deployment offers the same curations, so a name somebody
    // saved on one engine means the same question on the other.
    expect(datafusion.QUERY_EXAMPLES.map((example) => example.name)).toEqual(
      QUERY_EXAMPLES.map((example) => example.name),
    );
    for (const example of datafusion.QUERY_EXAMPLES) {
      expect(example.query).not.toContain('data_frame()');
      expect(example.query.trimEnd().endsWith(')')).toBe(true);
    }
    // And the title pattern survived being written in TypeScript for Python.
    const titanic = datafusion.QUERY_EXAMPLES.find(
      (example) => example.name === 'titanic/passengers',
    );
    expect(titanic?.query).toContain('lit(r"^[^,]*,\\s*([^.]+)\\..*$"), lit(r"\\1")');
  });

  it('ships a DataFusion variant of the seed pipelines, each naming its engine', () => {
    const variants = examples.filter((example) => example.name.endsWith('-datafusion'));

    expect(variants).toHaveLength(5);
    for (const variant of variants) {
      const transforms = variant.blocks.filter((block) => block.spec.kind === 'transform');
      expect(
        transforms.every(
          (block) => block.spec.kind === 'transform' && block.spec.engine === 'datafusion',
        ),
      ).toBe(true);
    }
    const features = variants.find(
      (variant) => variant.name === 'curation/titanic-features-datafusion',
    );
    const chain = orderOf(features?.blocks ?? [], features?.edges ?? []);
    expect(compilePython(chain ?? [])).toContain(
      'df = read("hub_rows", dataset="phihung/titanic", limit="100", split="train")',
    );
  });

  it('has a DuckDB counterpart for every example, under the same name', () => {
    expect(duckdb.QUERY_EXAMPLES.map((example) => example.name)).toEqual(
      QUERY_EXAMPLES.map((example) => example.name),
    );
    for (const example of duckdb.QUERY_EXAMPLES) {
      expect(example.query).not.toContain('data_frame()');
      // DataFusion's names are not DuckDB's: a query written in one is refused by the other.
      expect(example.query).not.toMatch(/\b(col|lit)\(/);
      expect(example.query.trimEnd().endsWith(')')).toBe(true);
    }
    const titanic = duckdb.QUERY_EXAMPLES.find((example) => example.name === 'titanic/passengers');
    expect(titanic?.query).toContain(
      'ConstantExpression(r"^[^,]*,\\s*([^.]+)\\..*$"),\n            ConstantExpression(r"\\1")',
    );
  });

  it('ships a DuckDB variant of the seed pipelines, each naming its engine', () => {
    const variants = examples.filter((example) => example.name.endsWith('-duckdb'));

    expect(variants).toHaveLength(5);
    for (const variant of variants) {
      const transforms = variant.blocks.filter((block) => block.spec.kind === 'transform');
      expect(
        transforms.every(
          (block) => block.spec.kind === 'transform' && block.spec.engine === 'duckdb',
        ),
      ).toBe(true);
    }
    const features = variants.find(
      (variant) => variant.name === 'curation/titanic-features-duckdb',
    );
    const chain = orderOf(features?.blocks ?? [], features?.edges ?? []);
    expect(compilePython(chain ?? [])).toContain(
      'df = read("hub_rows", dataset="phihung/titanic", limit="100", split="train")',
    );
  });

  it('checks for a missing age before it compares one', () => {
    // A fifth of the corpus reports no age, and `between()` throws on a null
    // rather than answering false — so the null branch has to be outermost or
    // the example fails on the real data it names.
    const passengers = QUERY_EXAMPLES.find((example) => example.name === 'titanic/passengers');
    const band = passengers?.query.indexOf("->withEntry('age_band'") ?? -1;
    const nullCheck = passengers?.query.indexOf("ref('age')->isNull()") ?? -1;
    const comparison = passengers?.query.indexOf("ref('age')->lessThan") ?? -1;

    expect(band).toBeGreaterThan(-1);
    expect(nullCheck).toBeGreaterThan(band);
    expect(comparison).toBeGreaterThan(nullCheck);
  });
});
