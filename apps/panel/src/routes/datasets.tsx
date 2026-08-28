import { createFileRoute } from '@tanstack/react-router';

import { AreaPlaceholder } from '@/components/area-placeholder';

export const Route = createFileRoute('/datasets')({
  component: DatasetsPage,
});

function DatasetsPage() {
  return (
    <AreaPlaceholder
      title="Datasets"
      summary="The cases evaluation runs against, most of them harvested from real runs rather than written by hand."
      sections={[
        {
          title: 'Collections',
          description:
            'Versioned sets of input/expected pairs. Versioned because a score is only comparable against the version it was measured on.',
        },
        {
          title: 'Capture from runs',
          description:
            'Promote a span from the explorer into a case. A production failure becomes a regression test in one step, which is the whole reason the two areas sit in one product.',
        },
        {
          title: 'Curation',
          description: 'Deduplicate, label and split. What separates a dataset from a log dump.',
        },
      ]}
      blockedOn={
        <>
          Building a dataset is an ETL job, not a request: read the exported runs, filter,
          deduplicate, write a versioned artifact. That is the Flow PHP side of the split — it reads
          Parquet the projector exports and writes dataset artifacts the Rust API serves. Neither
          the export nor the artifact store exists yet, so the capture button in the explorer has
          nowhere to write to.
          <br />
          <br />
          What already exists is the <em>name</em>. An evaluation report carries the{' '}
          <code>dataset</code> it was measured on, and the Evaluation area refuses to compare two
          reports that disagree about it. So a dataset is a string a producer supplies today and an
          artifact this area will own later — the identifier does not change when it becomes one.
        </>
      }
    />
  );
}
