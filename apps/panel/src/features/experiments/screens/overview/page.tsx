import { getRouteApi } from '@tanstack/react-router';

import { AreaPlaceholder } from '@/shared/components/area-placeholder';
import { Badge, Card } from '@/shared/components/ui/primitives';

/**
 * Experiments: changing the thing being observed.
 *
 * What belongs here is the comparison — a variant pinned to the traces it
 * produced — and it does not exist yet, so `AreaPlaceholder` names exactly what
 * is missing, because a plausible fake reads as working software. Starting the
 * work is not this page's: a registered workflow runs as a managed execution,
 * and it is followed in Workflows. The launcher that used to sit here asked the
 * Flyte engine, which is gone (AW-4).
 */

const routeApi = getRouteApi('/experiments');

export function ExperimentsPage() {
  const search = routeApi.useSearch();

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="text-lg font-semibold">Experiments</h1>
        <p className="max-w-3xl text-sm text-muted-foreground">
          Changing the thing being observed: prompt variants, model swaps, fine-tuning runs, and
          what each of them cost. Run the work as a managed workflow, watch it in Workflows and
          judge it in Evaluation.
        </p>
      </div>

      {search.dataset ? (
        <Card className="flex flex-wrap items-center gap-2 p-3 text-xs">
          <span className="text-muted-foreground">Experiment scope</span>
          <Badge>{search.dataset}</Badge>
          {search.variant ? <Badge tone="warning">{search.variant}</Badge> : null}
          <span className="text-muted-foreground">
            This URL pins the dataset version and variant a comparison is about.
          </span>
        </Card>
      ) : null}

      <AreaPlaceholder
        title="Comparison"
        summary="Two variants side by side: quality from evaluation, latency and token cost from the traces already recorded here."
        sections={[
          {
            title: 'Variants',
            description:
              'A change under test, pinned to the dataset and suite it was measured on. Without that pinning, two numbers are not a comparison.',
          },
          {
            title: 'Training runs',
            description:
              "Loss curves and checkpoints for a variant's training runs, linked to the model id that later shows up on production spans.",
          },
          {
            title: 'Cost',
            description:
              'What a variant cost to produce and to serve, joined from the spans its runs published.',
          },
        ]}
        blockedOn={
          <>
            Running the work is not what is missing: a registered workflow runs as a managed
            execution and is watchable in Workflows. The missing join is from a <code>variant</code>{' '}
            to the traces it produced, where latency and token cost live. <code>model</code> and{' '}
            <code>workflow</code> are dimensions today; <code>variant</code> must become one before
            a comparison can combine quality, latency and cost without guessing.
            <br />
            <br />
            Loss curves need their own retained record rather than being squeezed into either a
            trace or an evaluation report.
          </>
        }
      />
    </div>
  );
}
