import * as React from 'react';
import { useLocalScope } from '@/shared/lib/local-views';

/** Local preferences use the same instance/identity boundary as saved views. */
export function Appearance() {
  const scope = useLocalScope();
  const key = scope ? `${scope}:appearance` : undefined;
  const [theme, setTheme] = React.useState('dark');
  const [density, setDensity] = React.useState('comfortable');
  const [message, setMessage] = React.useState('');
  React.useEffect(() => {
    let nextTheme = 'dark',
      nextDensity = 'comfortable';
    setMessage('');
    try {
      const saved = key ? JSON.parse(window.localStorage.getItem(key) ?? 'null') : null;
      if (saved?.schema_version === 1) {
        if (saved.theme === 'light') nextTheme = 'light';
        if (saved.density === 'compact') nextDensity = 'compact';
      }
    } catch {
      setMessage('Could not read local preferences.');
    }
    setTheme(nextTheme);
    setDensity(nextDensity);
  }, [key]);
  React.useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.density = density;
  }, [theme, density]);
  function save(nextTheme: string, nextDensity: string) {
    setTheme(nextTheme);
    setDensity(nextDensity);
    try {
      if (!key) throw new Error('identity unavailable');
      window.localStorage.setItem(
        key,
        JSON.stringify({ schema_version: 1, theme: nextTheme, density: nextDensity }),
      );
      setMessage('Saved on this device.');
    } catch {
      setMessage('Applied for this session; could not save locally.');
    }
  }
  return (
    <details className="relative text-xs">
      <summary className="cursor-pointer rounded px-2 py-1">Appearance</summary>
      <div className="absolute right-0 z-30 mt-2 grid w-60 gap-3 rounded border border-border bg-card p-3 shadow-lg">
        <p>Preferences on this device</p>
        <label>
          Theme{' '}
          <select
            className="ml-2 rounded border p-1"
            value={theme}
            onChange={(event) => save(event.target.value, density)}
          >
            <option value="dark">Dark</option>
            <option value="light">Light</option>
          </select>
        </label>
        <label>
          Table density{' '}
          <select
            className="ml-2 rounded border p-1"
            value={density}
            onChange={(event) => save(theme, event.target.value)}
          >
            <option value="comfortable">Comfortable</option>
            <option value="compact">Compact</option>
          </select>
        </label>
        <p role="status">{message}</p>
      </div>
    </details>
  );
}
