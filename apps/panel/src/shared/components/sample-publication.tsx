import { Link } from '@tanstack/react-router';
import type { DatasetSample, PublishedDataset } from '@/api/generated/types.gen';
import { Badge, Button, Card, Spinner } from '@/shared/components/ui/primitives';

export function SamplePublication({ name, rowCount, sample, pending, disabled, error, published, onPublish }: {
  name: string;
  rowCount: number;
  sample: DatasetSample;
  pending: boolean;
  disabled: boolean;
  error: Error | null;
  published?: PublishedDataset;
  onPublish: () => void;
}) {
  return <Card className="flex flex-col gap-3 border-warning/40 p-3">
    <div className="flex flex-wrap items-center gap-2">
      <Badge tone="warning">{sample.mode === 'preview' ? 'Preview sample' : 'Truncated sample'}</Badge>
      <span className="text-sm">{rowCount} rows available</span>
    </div>
    <p className="text-sm">
      Publish these rows as a sample to <strong className="break-all">{name}</strong>.
      {' '}This is limited output, not a random or representative sample.
    </p>
    {sample.truncated_stages.length ? <p className="break-words text-xs text-muted-foreground">
      Truncated stages: {sample.truncated_stages.join(', ')}.
    </p> : null}
    <div className="flex flex-wrap items-center gap-3">
      <Button variant="outline" className="min-h-11" disabled={disabled || pending} onClick={onPublish}>
        {pending ? <Spinner /> : null} Publish sample
      </Button>
      {disabled && !pending ? <span className="text-xs text-muted-foreground">Finish editing and run again if the result is out of date.</span> : null}
    </div>
    {error ? <p role="alert" className="text-sm text-danger">{error.message}</p> : null}
    {published ? <p role="status" className="text-sm">
      Sample published: <Link to="/datasets" search={{ dataset: published.dataset.name, version: published.dataset.latest.version }}
        className="break-all text-primary underline">{published.dataset.name}@{published.dataset.latest.version.slice(0, 12)}</Link>
      {' '}· {published.dataset.latest.row_count} rows.
    </p> : null}
  </Card>;
}
