import type { PipelineBlock } from '@/api/generated/types.gen';
import { BlockInspector } from '@/components/block-inspector';
import { Badge, Button } from '@/components/ui/primitives';
import type { BlockResult, PipelineOutcomes } from '@/lib/pipeline';

/** One editor for the same graph, with actual prefix results below each cell. */
export function PipelineNotebook({
  chain,
  outcomes,
  results,
  busy,
  locked,
  onChange,
  onDelete,
  onRunTo,
  onDirtyChange,
  onPublish,
  canPublish,
}: {
  chain: PipelineBlock[];
  outcomes: PipelineOutcomes;
  results: Record<string, BlockResult>;
  busy: boolean;
  locked: boolean;
  onChange: (block: PipelineBlock) => void;
  onDelete: (id: string) => void;
  onRunTo: (id: string) => void;
  onDirtyChange: (id: string, dirty: boolean) => void;
  onPublish: () => void;
  canPublish: boolean;
}) {
  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-6" aria-label="Flow notebook">
      <p className="text-sm text-muted-foreground">
        Each cell edits a block in this flow. Run to here executes its preceding cells and shows the
        output at each step. PHP cells query the source through that step; Python cells receive the
        preceding output. Preview never publishes data.
      </p>
      {chain.map((block, index) => {
        const outcome = outcomes[block.id];
        const result = results[block.id];
        return (
          <section
            key={block.id}
            aria-label={`Cell ${index + 1}: ${block.title}`}
            className="flex min-w-0 flex-col gap-2"
          >
            <div className="flex flex-wrap items-center gap-2">
              <Badge>{index + 1}</Badge>
              <h2 className="font-semibold">{block.title}</h2>
              <span className="text-xs text-muted-foreground">
                {outcome?.status === 'running'
                  ? 'Running…'
                  : result
                    ? `${result.rowCount} rows`
                    : 'Not run'}
              </span>
              <Button
                className="ml-auto"
                size="sm"
                variant="outline"
                disabled={locked}
                onClick={() => onRunTo(block.id)}
              >
                Run to here
              </Button>
            </div>
            <BlockInspector
              block={block}
              disabled={busy}
              onChange={onChange}
              onDelete={() => onDelete(block.id)}
              onDirtyChange={onDirtyChange}
            />
            {outcome?.status === 'failed' ? (
              <div role="alert" className="rounded-md border border-danger p-3 text-sm text-danger">
                {outcome.message}
                {outcome.stderr ? (
                  <pre className="mt-2 overflow-auto whitespace-pre-wrap text-xs">
                    {outcome.stderr}
                  </pre>
                ) : null}
              </div>
            ) : null}
            {result ? (
              <div className="overflow-hidden rounded-md border border-border">
                <p className="border-b border-border bg-muted/30 p-2 text-xs">
                  Output after this cell · {result.rowCount} rows · showing up to 25
                </p>
                {result.stdout ? (
                  <pre className="max-h-48 overflow-auto whitespace-pre-wrap p-3 text-xs">
                    {result.stdout}
                  </pre>
                ) : null}
                <div className="max-h-80 overflow-auto">
                  <table className="w-full text-left text-xs">
                    <thead className="sticky top-0 bg-background">
                      <tr>
                        {result.columns.map((column) => (
                          <th className="border-b border-border px-3 py-2" key={column}>
                            {column}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {result.rows.map((row, rowIndex) => (
                        <tr key={rowIndex}>
                          {result.columns.map((column) => {
                            const value =
                              typeof row[column] === 'object'
                                ? JSON.stringify(row[column])
                                : String(row[column] ?? '');
                            return (
                              <td
                                key={column}
                                className="max-w-72 truncate border-b border-border/40 px-3 py-2"
                                title={value}
                              >
                                {value}
                              </td>
                            );
                          })}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
                <details className="border-t border-border p-2 text-xs">
                  <summary className="cursor-pointer">
                    Inspect complete values in these 25 rows
                  </summary>
                  <pre className="max-h-96 overflow-auto p-2">
                    {JSON.stringify(result.rows, null, 2)}
                  </pre>
                </details>
              </div>
            ) : null}
            {block.spec.kind === 'view' ? (
              <div className="flex items-center gap-3">
                <Button disabled={locked || !canPublish} onClick={onPublish}>
                  Publish dataset
                </Button>
                <span className="text-xs text-muted-foreground">
                  Run the whole flow first; publishing saves the definition and its output.
                </span>
              </div>
            ) : null}
          </section>
        );
      })}
    </div>
  );
}
