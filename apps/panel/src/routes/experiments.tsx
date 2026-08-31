import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

import { AreaPlaceholder } from '@/components/area-placeholder';
import { Badge, Card } from '@/components/ui/primitives';

const searchSchema = z.object({
  dataset: z.string().optional(),
  variant: z.string().optional(),
});

export const Route = createFileRoute('/experiments')({
  validateSearch: searchSchema,
  component: ExperimentsPage,
});

function ExperimentsPage() {
  const search = Route.useSearch();
  return (
    <div className="flex flex-col gap-4">
      {search.dataset ? (
        <Card className="flex flex-wrap items-center gap-2 p-3 text-xs">
          <span className="text-muted-foreground">Planned experiment scope</span>
          <Badge>{search.dataset}</Badge>
          {search.variant ? <Badge tone="warning">{search.variant}</Badge> : null}
          <span className="text-muted-foreground">
            This URL already pins the dataset version and variant that the future experiment record must retain.
          </span>
        </Card>
      ) : null}
      <AreaPlaceholder
        title="Experiments"
        summary="Changing the thing being observed: prompt variants, model swaps, fine-tuning runs, and what each of them cost."
        sections={[
          {
            title: 'Variants',
            description:
              'A change under test, pinned to the dataset and suite it was measured on. Without that pinning, two numbers are not a comparison.',
          },
          {
            title: 'Training runs',
            description:
              'Fine-tuning jobs with their loss curves and checkpoints, linked to the model id that later shows up on production spans.',
          },
          {
            title: 'Comparison',
            description:
              'Two variants side by side: quality from evaluation, latency and token cost from the traces already recorded here.',
          },
        ]}
        blockedOn={
          <>
            Dataset lineage now provides the stable <code>dataset@version</code> and evaluation
            reports already carry a <code>variant</code>. The remaining join is from that variant to
            the traces it produced, where latency and token cost live. <code>model</code> and{' '}
            <code>workflow</code> are dimensions today; <code>variant</code> must become one before an
            experiment comparison can combine quality, latency and cost without guessing.
            <br />
            <br />
            Training runs are further out: loss curves and checkpoints need their own retained
            record rather than being squeezed into either a trace or an evaluation report.
          </>
        }
      />
    </div>
  );
}
