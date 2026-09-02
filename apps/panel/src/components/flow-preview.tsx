import * as React from 'react';

import { Badge, Card, EmptyState } from '@/components/ui/primitives';
import { VirtualList } from '@/components/virtual-list';
import { FlowQueryError, FlowUnavailableError, type FlowCheck, type FlowResult } from '@/lib/flow';
import { cn, formatCount } from '@/lib/utils';

export function FlowDiagnostics({
  check,
  pending = false,
}: {
  check?: FlowCheck;
  pending?: boolean;
}) {
  if (!check) return null;
  if (check.ok) {
    return (
      <p className="px-1 text-xs text-muted-foreground">
        {pending ? 'checking…' : `checks out · ${check.checked_by.join(' + ')}`}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-1.5">
      {check.diagnostics.map((diagnostic, index) => (
        <div
          key={`${diagnostic.offset}-${index}`}
          className="rounded-md border border-warning/40 bg-warning/5 px-3 py-2"
        >
          <p className="text-sm">{diagnostic.message}</p>
          <p className="mt-1 text-[11px] text-muted-foreground">
            at character {diagnostic.offset}
            {diagnostic.help ? ` · ${diagnostic.help}` : ''}
          </p>
        </div>
      ))}
    </div>
  );
}

export function FlowResultView({
  result,
  error,
  emptyTitle = 'Nothing simulated yet',
  previewImages = false,
}: {
  result?: FlowResult;
  error?: Error | null;
  emptyTitle?: string;
  /**
   * Draw https cells as pictures.
   *
   * Off by default and on where a table is known to be about images. A run's
   * or a span's URL column is an address somebody copies, and turning every
   * one of those into a network request the reader did not ask for is a
   * different tab's decision to make.
   */
  previewImages?: boolean;
}) {
  if (error instanceof FlowUnavailableError) {
    return (
      <EmptyState
        title="The Flow service stopped responding"
        hint="Start it with `just flow-serve`."
      />
    );
  }
  if (error instanceof FlowQueryError) {
    return (
      <Card className="border-danger/40 p-4">
        <p className="text-xs font-medium text-danger">The pipeline was refused</p>
        <p className="mt-1 text-sm">{error.message}</p>
        {error.column > 0 ? (
          <p className="mt-2 text-xs text-muted-foreground">at character {error.column}</p>
        ) : null}
      </Card>
    );
  }
  if (error)
    return <Card className="border-danger/40 p-4 text-sm text-danger">{error.message}</Card>;
  if (!result) {
    return (
      <EmptyState
        title={emptyTitle}
        hint="Simulation reads data but never writes a dataset version."
      />
    );
  }
  if (result.rows.length === 0) {
    return <EmptyState title="No rows" hint="The pipeline ran successfully and matched nothing." />;
  }

  return (
    <Card className="overflow-hidden">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-2 text-xs text-muted-foreground">
        <span>{formatCount(result.row_count)} rows</span>
        <span>·</span>
        <span>
          {result.dataset} · {result.grain}
        </span>
        <span>·</span>
        <span>period: {formatPeriod(result.window_seconds)}</span>
        <span>·</span>
        <span>{result.took_ms} ms</span>
        {result.truncated ? <Badge tone="warning">preview cap reached</Badge> : null}
      </div>
      <div className="overflow-x-auto">
        <div style={{ minWidth: `max(100%, ${result.columns.length * 10}rem)` }}>
          <div className="flex border-b border-border bg-muted/40 text-xs font-medium">
            {result.columns.map((column) => (
              <div key={column} className="min-w-0 flex-1 px-3 py-1.5">
                {column}
              </div>
            ))}
          </div>
          <VirtualList
            items={result.rows}
            className="max-h-[24rem]"
            estimateSize={previewImages ? 48 : 30}
            keyOf={(_, index) => String(index)}
            renderRow={(row) => (
              <div className="flex border-b border-border/20 text-sm hover:bg-accent/30">
                {result.columns.map((column) => (
                  <div
                    key={column}
                    className={cn(
                      'min-w-0 flex-1 overflow-hidden px-3 py-1.5 tabular-nums',
                      result.truncate_cells ? 'truncate' : '[overflow-wrap:anywhere]',
                    )}
                  >
                    <Cell value={row[column]} preview={previewImages} />
                  </div>
                ))}
              </div>
            )}
          />
        </div>
      </div>
    </Card>
  );
}

function formatPeriod(seconds: number | null | undefined): string {
  if (!seconds) return 'all retained data';
  if (seconds % 604_800 === 0) return `last ${seconds / 604_800}w`;
  if (seconds % 86_400 === 0) return `last ${seconds / 86_400}d`;
  if (seconds % 3_600 === 0) return `last ${seconds / 3_600}h`;
  if (seconds % 60 === 0) return `last ${seconds / 60}m`;
  return `last ${seconds}s`;
}

function Cell({ value, preview = false }: { value: unknown; preview?: boolean }) {
  if (value === null || value === undefined)
    return <span className="text-muted-foreground">—</span>;
  if (typeof value === 'object') return <span className="id">{JSON.stringify(value)}</span>;
  if (preview && typeof value === 'string' && value.startsWith('https://')) {
    return <Thumbnail uri={value} />;
  }
  return <>{String(value)}</>;
}

/**
 * A cell that is a picture, with the address still readable underneath.
 *
 * Every https cell is tried rather than a column named `uri` or `image`: a
 * pipeline is free to rename its columns, and a table that only draws the one
 * spelling would stop drawing the moment somebody did. What decides is whether
 * the browser can decode it, which is the same question the reader has.
 *
 * A failure removes the picture and leaves the text. The row was always going
 * to say what it says; the image is the part that is extra.
 */
function Thumbnail({ uri }: { uri: string }) {
  const [broken, setBroken] = React.useState(false);
  return (
    <span className="flex items-center gap-2">
      {broken ? null : (
        <img
          src={uri}
          alt=""
          onError={() => setBroken(true)}
          className="h-10 w-10 shrink-0 rounded border border-border bg-muted/20 object-cover"
        />
      )}
      <span className="min-w-0 truncate">{uri}</span>
    </span>
  );
}
