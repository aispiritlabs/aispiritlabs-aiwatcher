import { createFileRoute } from '@tanstack/react-router';

import { AreaPlaceholder } from '@/components/area-placeholder';

export const Route = createFileRoute('/experiments')({
  component: ExperimentsPage,
});

function ExperimentsPage() {
  return (
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
          This area is downstream of the other two: a comparison is an evaluation result joined to
          the metrics an experiment produced. Half of that join now exists — an evaluation report
          carries the <code>variant</code> under test and compares itself against the previous
          report on the same dataset, so quality is answerable per variant. The other half is not:
          nothing yet joins a variant to the <em>traces</em> it produced, which is where its latency
          and its token cost live. <code>model</code> and <code>workflow</code> are dimensions the
          explorer groups by today; a variant is not one of them, and making it one is the next
          thing this area needs.
          <br />
          <br />
          Training runs are further out again. Loss curves and checkpoints are a different kind of
          record from either a trace or a report, and nothing in the backend holds one.
        </>
      }
    />
  );
}
