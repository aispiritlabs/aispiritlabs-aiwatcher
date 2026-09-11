import * as React from 'react';
import { Button } from '@/shared/components/ui/primitives';
import {
  readViews,
  searchFromView,
  useLocalScope,
  viewFromSearch,
  type LocalView,
  type ViewScreen,
} from '@/shared/lib/local-views';

export function LocalViews({
  screen,
  search,
  onRestore,
}: {
  screen: ViewScreen;
  search: Record<string, unknown>;
  onRestore: (search: Record<string, unknown>) => void;
}) {
  const scope = useLocalScope();
  const key = scope ? `${scope}:views` : undefined;
  const [views, setViews] = React.useState<LocalView[]>([]);
  const [loadedKey, setLoadedKey] = React.useState<string>();
  const [name, setName] = React.useState('');
  const [message, setMessage] = React.useState('');
  React.useEffect(() => {
    setViews([]);
    setLoadedKey(undefined);
    setName('');
    setMessage('');
    if (!key) return;
    const read = () => {
      try {
        setViews(readViews(window.localStorage.getItem(key)));
        setLoadedKey(key);
      } catch {
        setMessage('Local views cannot be read. Stored data was left unchanged.');
      }
    };
    read();
    const listener = (event: StorageEvent) => {
      if (event.key === key) read();
    };
    window.addEventListener('storage', listener);
    return () => window.removeEventListener('storage', listener);
  }, [key]);
  function save() {
    if (!key) return;
    try {
      const view = viewFromSearch(name, screen, search);
      const existing = readViews(window.localStorage.getItem(key));
      const next = [
        ...existing.filter((entry) => entry.screen !== screen || entry.name !== view.name),
        view,
      ];
      if (next.length > 30) throw new Error('limit');
      window.localStorage.setItem(key, JSON.stringify(next));
      setViews(next);
      setLoadedKey(key);
      setMessage(`Saved “${view.name}” on this device.`);
    } catch {
      setMessage(
        'Could not save. Use a name up to 80 characters; storage must be available (maximum 30 views).',
      );
    }
  }
  return (
    <section className="rounded border border-border p-3 text-xs" aria-label="Local views">
      <p className="mb-2 text-muted-foreground">
        Views on this device · na tym urządzeniu. A saved view does not preserve historical data.
      </p>
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          save();
        }}
      >
        <label>
          View name{' '}
          <input
            className="rounded border border-border bg-background p-1"
            value={name}
            maxLength={80}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <Button size="sm" type="submit" disabled={!key || !name.trim()}>
          Save view
        </Button>
        {key &&
          loadedKey === key &&
          views
            .filter((view) => view.screen === screen)
            .map((view) => (
              <Button
                key={view.name}
                size="sm"
                variant="outline"
                type="button"
                onClick={() => {
                  try {
                    onRestore(searchFromView(view));
                    setName(view.name);
                    setMessage(`Opened “${view.name}”.`);
                  } catch {
                    setMessage('This view is not compatible with this screen.');
                  }
                }}
              >
                {view.name}
              </Button>
            ))}
      </form>
      {!key && <p>Local views are unavailable until identity is known.</p>}
      <p role="status">{message}</p>
    </section>
  );
}
