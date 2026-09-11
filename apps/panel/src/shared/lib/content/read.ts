import type { EngineContent } from '@/shared/lib/engine-content';
import type { QueryExample } from '@/shared/lib/flow';

/**
 * One Python engine's shipped content, as `content/<engine>.json` holds it (AW-3).
 *
 * The file is the one copy of that text. The panel reads it through here, and the
 * engine's own tests read the same bytes — `test_<engine>_shipped_content.py`, through
 * `pieces` in `services/query/contract/tests/conftest.py` — and admit every piece of it
 * under `strict`, so what the panel offers and what the engine runs cannot drift apart.
 * A copy in each language would be two lists free to disagree.
 *
 * Three rules of the file's own: a text of several lines is written as its lines,
 * joined with newlines here and there alike; the starter curation names the example it
 * is rather than repeating it; and the block inspector's help holds `{example}` where
 * the transform it shows goes, so that transform is a piece the engine checks rather
 * than prose nobody does.
 */
export interface ContentFile {
  starterQuery: string[];
  /** The `name` of the example that is also the starter curation. */
  starterCuration: string;
  newTransform: string;
  transformHelp: string;
  transformExample: string;
  transformations: { label: string; code: string; help: string }[];
  examples: (Omit<QueryExample, 'query'> & { query: string[] })[];
}

/** The content as the panel shows it; `file` names the JSON in a refusal. */
export function readContent(content: ContentFile, file: string): EngineContent {
  const examples = content.examples.map(({ query, ...example }) => ({
    ...example,
    query: query.join('\n'),
  }));
  const curation = examples.find((example) => example.name === content.starterCuration);
  if (!curation) {
    throw new Error(
      `${file}: starterCuration names ${content.starterCuration}, which is none of its examples`,
    );
  }
  return {
    starterQuery: content.starterQuery.join('\n'),
    starterCuration: curation.query,
    examples,
    transformations: content.transformations.map(
      ({ label, code, help }) => [label, code, help] as const,
    ),
    newTransform: content.newTransform,
    // A function, so a `$` in the transform is text rather than a replacement pattern.
    transformHelp: content.transformHelp.replace('{example}', () => content.transformExample),
  };
}
