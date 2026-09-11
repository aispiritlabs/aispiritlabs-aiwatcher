import { describe, expect, it } from 'vitest';

import datafusionFile from '@/shared/lib/content/datafusion.json';
import duckdbFile from '@/shared/lib/content/duckdb.json';
import { readContent, type ContentFile } from '@/shared/lib/content/read';

/**
 * Whether a piece of this content runs is the engine's answer, and
 * `test_<engine>_shipped_content.py` asks it of every piece under `strict`. What
 * is checkable here is the reading: that the panel shows the text the engine was
 * asked about, and refuses a file whose rules do not hold rather than showing
 * half of it.
 */
describe('a Python engine’s shipped content', () => {
  const files: ReadonlyArray<readonly [string, ContentFile]> = [
    ['duckdb', duckdbFile],
    ['datafusion', datafusionFile],
  ];

  it.each(files)('%s shows the transform the engine admits in its help', (_engine, file) => {
    const content = readContent(file, `content/${_engine}.json`);

    expect(content.transformHelp).toContain(file.transformExample);
    expect(content.transformHelp).not.toContain('{example}');
  });

  it.each(files)('%s starts a curation from one of its own examples', (_engine, file) => {
    const content = readContent(file, `content/${_engine}.json`);

    expect(content.examples.map((example) => example.query)).toContain(content.starterCuration);
  });

  it('refuses a starter curation that names none of its examples', () => {
    const file: ContentFile = { ...duckdbFile, starterCuration: 'nobody/saved-this' };

    expect(() => readContent(file, 'content/duckdb.json')).toThrow(
      'content/duckdb.json: starterCuration names nobody/saved-this',
    );
  });
});
