import { collapseUnchanged, diffLines, diffStat } from '@/lib/diff';

/**
 * Two prompt versions, with what changed between them.
 *
 * Rendered as a unified diff rather than side by side: prompt lines are prose
 * and wrap, and two wrapping columns at half width turn every paragraph into a
 * column of two-word lines.
 */
export function PromptDiff({ before, after }: { before: string; after: string }) {
  const lines = diffLines(before, after);
  if (!lines) {
    return (
      <p className="p-4 text-xs text-muted-foreground">
        These versions are too long to diff in the browser. The texts differ; fetch both versions to
        compare them.
      </p>
    );
  }
  const stat = diffStat(lines);
  if (stat.added === 0 && stat.removed === 0) {
    return <p className="p-4 text-xs text-muted-foreground">Identical.</p>;
  }

  return (
    <div>
      <div className="flex items-center gap-3 border-b border-border px-4 py-2 text-xs tabular-nums">
        <span className="text-success">+{stat.added}</span>
        <span className="text-danger">−{stat.removed}</span>
        <span className="text-muted-foreground">lines</span>
      </div>
      <div className="overflow-x-auto">
        <pre className="min-w-full text-xs leading-relaxed">
          {collapseUnchanged(lines).map((entry, index) =>
            'skipped' in entry ? (
              <div
                key={`skip-${index}`}
                className="border-y border-border/50 bg-muted/30 px-4 py-1 text-[11px] text-muted-foreground"
              >
                {entry.skipped} unchanged {entry.skipped === 1 ? 'line' : 'lines'}
              </div>
            ) : (
              <div
                key={`${entry.kind}-${entry.number}-${index}`}
                className={
                  entry.kind === 'added'
                    ? 'bg-success/10 px-4 text-foreground'
                    : entry.kind === 'removed'
                      ? 'bg-danger/10 px-4 text-foreground'
                      : 'px-4 text-muted-foreground'
                }
              >
                <span className="mr-3 select-none text-muted-foreground">
                  {entry.kind === 'added' ? '+' : entry.kind === 'removed' ? '−' : ' '}
                </span>
                <span className="whitespace-pre-wrap">{entry.text || ' '}</span>
              </div>
            ),
          )}
        </pre>
      </div>
    </div>
  );
}
