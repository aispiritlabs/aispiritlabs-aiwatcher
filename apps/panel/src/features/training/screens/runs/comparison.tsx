import type { TrainingRun } from '@/api/generated';
import { LearningCurve } from '@/shared/components/charts/learning-curve';
import { ParameterComparison } from '@/shared/components/parameter-comparison';
import { Card } from '@/shared/components/ui/primitives';

export function metricSeries(run: TrainingRun, metric: string) {
  return {
    key: run.run_id,
    label: run.run_id,
    points: (run.epochs ?? [])
      .filter((epoch) => typeof epoch.metrics[metric] === 'number')
      .map((epoch) => [epoch.epoch, epoch.metrics[metric]!] as [number, number]),
    missing: (run.epochs ?? [])
      .filter((epoch) => typeof epoch.metrics[metric] !== 'number')
      .map((epoch) => epoch.epoch),
  };
}

export function RunComparison({
  runs,
  selected,
  onSelect,
}: {
  runs: TrainingRun[];
  selected?: string[];
  onSelect: (metrics: string[]) => void;
}) {
  const names = [
    ...new Set(
      runs.flatMap((run) => (run.epochs ?? []).flatMap((epoch) => Object.keys(epoch.metrics))),
    ),
  ].sort();
  const shown = selected?.length ? selected : names;
  return (
    <div className="space-y-3">
      <Card className="overflow-auto p-4">
        <h2 className="text-base font-semibold">Training comparison</h2>
        <p className="text-xs text-muted-foreground">
          One axis per metric, by epoch. Best is the trainer’s reported selection; metric direction
          is not inferred.
        </p>
        <table className="mt-3 w-full text-left text-xs">
          <thead>
            <tr>
              {['Run', 'Status', 'Dataset', 'Best reported', 'Last measurement'].map((name) => (
                <th scope="col" className="p-2" key={name}>
                  {name}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {runs.map((run) => (
              <tr key={run.run_id} className="border-t border-border">
                <th scope="row" className="p-2">
                  {run.run_id}
                </th>
                <td className="p-2">{run.status}</td>
                <td className="p-2">
                  {run.dataset}
                  {!run.reproducible && ' · unversioned'}
                </td>
                <td className="p-2">
                  {run.best
                    ? `${run.best.metric}: ${run.best.value} (epoch ${run.best.epoch ?? '—'})`
                    : '—'}
                </td>
                <td className="p-2">
                  {shown.map((name) => {
                    const point = metricSeries(run, name).points.at(-1);
                    return (
                      <div key={name}>
                        {name}: {point ? `${point[1]} (epoch ${point[0]})` : '—'}
                      </div>
                    );
                  })}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>
      <ParameterComparison
        entries={runs.map((run) => ({ id: run.run_id, params: run.params ?? {} }))}
      />
      <fieldset className="flex flex-wrap gap-3 text-xs">
        <legend>Metrics</legend>
        {names.map((name) => (
          <label key={name}>
            <input
              type="checkbox"
              checked={shown.includes(name)}
              onChange={() =>
                onSelect(
                  shown.includes(name) && shown.length > 1
                    ? shown.filter((entry) => entry !== name)
                    : [...new Set([...shown, name])],
                )
              }
            />{' '}
            {name}
          </label>
        ))}
      </fieldset>
      {shown.map((name) => (
        <Card className="p-4" key={name}>
          <h3 className="text-sm font-semibold">{name}</h3>
          <LearningCurve series={runs.map((run) => metricSeries(run, name))} />
          {runs
            .filter((run) => metricSeries(run, name).points.length === 0)
            .map((run) => (
              <p key={run.run_id} className="text-xs text-muted-foreground">
                {run.run_id}: no {name} measurements.
              </p>
            ))}
        </Card>
      ))}
    </div>
  );
}
