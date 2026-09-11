import { Card } from '@/shared/components/ui/primitives';

function display(value: unknown): string {
  if (value === undefined) return '—';
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

export function ParameterComparison({
  entries,
}: {
  entries: { id: string; params: Record<string, unknown> }[];
}) {
  const names = [...new Set(entries.flatMap((entry) => Object.keys(entry.params)))].sort();
  return (
    <Card className="overflow-auto p-4">
      <h3 className="mb-2 text-sm font-semibold">Parameters</h3>
      <table className="w-full text-left text-xs">
        <caption>Different values are marked “differs”; — means not recorded.</caption>
        <thead>
          <tr>
            <th scope="col">Parameter</th>
            {entries.map((entry) => (
              <th scope="col" className="p-2" key={entry.id}>
                {entry.id}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {names.map((name) => (
            <tr className="border-t border-border" key={name}>
              <th scope="row" className="p-2">
                {name}
                {new Set(entries.map((entry) => display(entry.params[name]))).size > 1
                  ? ' · differs'
                  : ''}
              </th>
              {entries.map((entry) => (
                <td className="max-w-64 break-words p-2 font-mono" key={entry.id}>
                  {display(entry.params[name])}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
      {!names.length && <p className="text-xs text-muted-foreground">No parameters recorded.</p>}
    </Card>
  );
}
