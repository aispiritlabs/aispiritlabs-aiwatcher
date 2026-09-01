import { createFileRoute } from '@tanstack/react-router';
import { z } from 'zod';

import { AreaPlaceholder } from '@/components/area-placeholder';
import { EngineLauncher } from '@/components/engine-launcher';
import { DEFAULT_WINDOW_SECONDS, TimeRange, windowSearchSchema } from '@/components/time-range';
import type { PipelineStage } from '@/api/generated/types.gen';
import { Badge, Button, Card } from '@/components/ui/primitives';

/**
 * Experiments: changing the thing being observed.
 *
 * Two halves, and only one of them exists yet. The half that does is the
 * orchestrated one — training, evaluation and inference are workflows somebody
 * registered, and starting one from here is the same three questions Data
 * Curation asks about a dataset: which workflow, over what, for how long.
 *
 * The half that does not is the comparison: pinning a variant to the traces it
 * produced. `AreaPlaceholder` below names exactly what is missing, because a
 * plausible fake reads as working software.
 */

/**
 * The three stages this area starts. Curation is the fourth and lives on its
 * own page, because it produces the thing the other three consume.
 */
type ExperimentStage = Extract<PipelineStage, 'training' | 'evaluation' | 'inference'>;

const STAGES: { stage: ExperimentStage; label: string; summary: string }[] = [
  {
    stage: 'training',
    label: 'Training',
    summary:
      'Fine-tuning and training runs the orchestrator holds. Point one at a dataset version and it becomes a model id that later shows up on production spans.',
  },
  {
    stage: 'evaluation',
    label: 'Evaluation',
    summary:
      'Scoring a variant against a suite. The report it publishes lands in Evaluation on the same log as everything else.',
  },
  {
    stage: 'inference',
    label: 'Inference',
    summary:
      'Batch scoring and embedding jobs — the third leg, and the one whose cost shows up in Observability rather than here.',
  },
];

const searchSchema = z.object({
  ...windowSearchSchema,
  dataset: z.string().optional(),
  variant: z.string().optional(),
  stage: z.enum(['training', 'evaluation', 'inference']).optional(),
  engine: z.string().optional(),
  engineFind: z.string().optional(),
});

export const Route = createFileRoute('/experiments')({
  validateSearch: searchSchema,
  component: ExperimentsPage,
});

function ExperimentsPage() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const stage = search.stage ?? 'training';
  const windowSeconds = search.window ?? DEFAULT_WINDOW_SECONDS;
  const chosen = STAGES.find((option) => option.stage === stage) ?? STAGES[0]!;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold">Experiments</h1>
          <p className="max-w-3xl text-sm text-muted-foreground">
            Changing the thing being observed: prompt variants, model swaps, fine-tuning runs, and
            what each of them cost. Start the work here; watch it in Workflows and judge it in
            Evaluation.
          </p>
        </div>
        <TimeRange
          value={windowSeconds}
          onChange={(window) => void navigate({ search: (previous) => ({ ...previous, window }) })}
        />
      </div>

      {search.dataset ? (
        <Card className="flex flex-wrap items-center gap-2 p-3 text-xs">
          <span className="text-muted-foreground">Experiment scope</span>
          <Badge>{search.dataset}</Badge>
          {search.variant ? <Badge tone="warning">{search.variant}</Badge> : null}
          <span className="text-muted-foreground">
            This URL pins the dataset version and variant, and fills the workflow's inputs below.
          </span>
        </Card>
      ) : null}

      {/* One launcher, three stages. The stage is a filter over the same
          catalog rather than three separate pickers: an orchestrator names its
          launch plans however it likes, and a stage nobody's names match would
          be an empty tab that looks broken. */}
      <div className="flex flex-wrap items-center gap-1">
        {STAGES.map((option) => (
          <Button
            key={option.stage}
            size="sm"
            variant={option.stage === stage ? 'default' : 'outline'}
            onClick={() =>
              void navigate({
                search: (previous) => ({ ...previous, stage: option.stage, engine: undefined }),
              })
            }
          >
            {option.label}
          </Button>
        ))}
      </div>

      <EngineLauncher
        stage={stage}
        title={`Run a registered ${chosen.label.toLowerCase()} workflow`}
        summary={chosen.summary}
        context={{ dataset: search.dataset, windowSeconds, values: { variant: search.variant } }}
        search={search.engineFind ?? ''}
        onSearchChange={(engineFind) =>
          void navigate({
            search: (previous) => ({ ...previous, engineFind: engineFind || undefined }),
            replace: true,
          })
        }
        selected={search.engine}
        onSelect={(engine) =>
          void navigate({ search: (previous) => ({ ...previous, engine }), replace: true })
        }
      />

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
              'Loss curves and checkpoints for the launches above, linked to the model id that later shows up on production spans.',
          },
          {
            title: 'Cost',
            description:
              'What a variant cost to produce and to serve, joined from the spans its runs published.',
          },
        ]}
        blockedOn={
          <>
            Launching is solved: a registered workflow is startable from here and its execution is
            watchable in Workflows. The remaining join is from a <code>variant</code> to the traces
            it produced, where latency and token cost live. <code>model</code> and{' '}
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
