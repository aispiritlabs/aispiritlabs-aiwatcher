/**
 * A line diff, for looking at what an optimiser did to a prompt.
 *
 * This is the question the prompt page exists to answer. An optimiser returns
 * a rewritten prompt, the scores say it is better, and the only way to know
 * *why* — or to notice that it quietly dropped the instruction about units — is
 * to see the two texts side by side. A score with no visible change behind it
 * is the thing people mean when they say they do not trust an optimiser.
 *
 * Plain Myers-style LCS over lines, computed in the browser. The alternative is
 * a diff library, which for prompts — tens of lines, not tens of thousands — is
 * a dependency for one function.
 */

export type DiffKind = 'same' | 'added' | 'removed';

export interface DiffLine {
  kind: DiffKind;
  text: string;
  /** 1-based line number on the side this line exists on. */
  number: number;
}

/**
 * Above this many lines the quadratic table stops being free.
 *
 * A prompt that long is a document, and the honest answer for it is "these
 * differ" rather than a diff computed for two seconds on the main thread.
 */
const MAX_LINES = 2_000;

export function diffLines(before: string, after: string): DiffLine[] | null {
  const left = before.split('\n');
  const right = after.split('\n');
  if (left.length > MAX_LINES || right.length > MAX_LINES) return null;

  // `lengths[i][j]` is the length of the longest common subsequence of
  // `left[i..]` and `right[j..]`. Filled backwards so the walk below can go
  // forwards, which is what keeps the output in reading order.
  //
  // The reads go through helpers because `noUncheckedIndexedAccess` is on:
  // every index is provably inside the table, and saying so with a `!` in six
  // places would be six places to be wrong later.
  const lengths: number[][] = Array.from({ length: left.length + 1 }, () =>
    new Array<number>(right.length + 1).fill(0),
  );
  const cell = (i: number, j: number): number => lengths[i]?.[j] ?? 0;
  const lineAt = (source: string[], index: number): string => source[index] ?? '';

  for (let i = left.length - 1; i >= 0; i -= 1) {
    const row = lengths[i];
    if (!row) continue;
    for (let j = right.length - 1; j >= 0; j -= 1) {
      row[j] =
        lineAt(left, i) === lineAt(right, j)
          ? cell(i + 1, j + 1) + 1
          : Math.max(cell(i + 1, j), cell(i, j + 1));
    }
  }

  const lines: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < left.length && j < right.length) {
    if (lineAt(left, i) === lineAt(right, j)) {
      lines.push({ kind: 'same', text: lineAt(left, i), number: j + 1 });
      i += 1;
      j += 1;
    } else if (cell(i + 1, j) >= cell(i, j + 1)) {
      lines.push({ kind: 'removed', text: lineAt(left, i), number: i + 1 });
      i += 1;
    } else {
      lines.push({ kind: 'added', text: lineAt(right, j), number: j + 1 });
      j += 1;
    }
  }
  for (; i < left.length; i += 1) {
    lines.push({ kind: 'removed', text: lineAt(left, i), number: i + 1 });
  }
  for (; j < right.length; j += 1) {
    lines.push({ kind: 'added', text: lineAt(right, j), number: j + 1 });
  }
  return lines;
}

export interface DiffStat {
  added: number;
  removed: number;
}

export function diffStat(lines: DiffLine[]): DiffStat {
  return lines.reduce<DiffStat>(
    (total, line) => ({
      added: total.added + (line.kind === 'added' ? 1 : 0),
      removed: total.removed + (line.kind === 'removed' ? 1 : 0),
    }),
    { added: 0, removed: 0 },
  );
}

/**
 * Collapse long stretches of unchanged lines, keeping `context` either side.
 *
 * A prompt is mostly unchanged between versions; without this the diff is a
 * wall of grey with two coloured lines somewhere in it.
 */
export function collapseUnchanged(lines: DiffLine[], context = 2): (DiffLine | { skipped: number })[] {
  const changedNear = new Set<number>();
  lines.forEach((line, index) => {
    if (line.kind === 'same') return;
    for (let at = index - context; at <= index + context; at += 1) changedNear.add(at);
  });

  const output: (DiffLine | { skipped: number })[] = [];
  let skipped = 0;
  lines.forEach((line, index) => {
    if (changedNear.has(index)) {
      if (skipped > 0) {
        output.push({ skipped });
        skipped = 0;
      }
      output.push(line);
    } else {
      skipped += 1;
    }
  });
  if (skipped > 0) output.push({ skipped });
  return output;
}
