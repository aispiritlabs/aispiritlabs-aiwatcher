import * as React from 'react';
import { useRouter } from '@tanstack/react-router';
import { z } from 'zod';
import { SECTIONS, areaOf, sectionOf } from '@/app/navigation';
import { useLocalScope } from '@/shared/lib/local-views';

export const START_PAGES = [
  { to: '/', label: 'Your work' },
  ...SECTIONS.flatMap((section) => section.areas.flatMap((area) => area.views.length
    ? area.views.map((view) => ({ to: view.to, label: `${area.label} · ${view.label}` }))
    : [{ to: area.to, label: area.label }])),
];

/** Only internal panel links; retain the full search and hash without rewriting stored views. */
export function isPanelHref(href: string): boolean {
  // The control characters are the point: a stored href carrying one is what
  // this refuses, so the rule that warns about writing one by accident has
  // nothing to say here.
  // eslint-disable-next-line no-control-regex
  if (!href.startsWith('/') || href.startsWith('//') || /[\\\s\u0000-\u001f]/.test(href)) return false;
  try {
    const url = new URL(href, 'https://panel.invalid');
    return url.origin === 'https://panel.invalid' && (
      START_PAGES.some((page) => page.to === url.pathname)
      || url.pathname === '/account'
      || url.pathname === '/account/access'
      || /^\/(runs|prompts)\/[^/]+$/.test(url.pathname)
    );
  } catch { return false; }
}

const preferencesSchema = z.object({
  schema_version: z.literal(1),
  shell: z.enum(['new', 'classic']),
  collapsed: z.boolean(),
  start: z.string().refine((value) => START_PAGES.some((page) => page.to === value)),
  pins: z.array(z.object({
    href: z.string().max(16_000).refine(isPanelHref),
    label: z.string().trim().min(1).max(80),
  })).max(20).refine((pins) => new Set(pins.map((pin) => pin.href)).size === pins.length),
});
export type NavigationPreferences = z.infer<typeof preferencesSchema>;
export type ShellMode = NavigationPreferences['shell'];
export type ShellPolicy = ShellMode | 'user';

export function shellPolicy(value: unknown): ShellPolicy {
  return value === 'classic' || value === 'new' ? value : 'user';
}

export function readNavigationPreferences(raw: string | null, legacySidebar: string | null = null): NavigationPreferences {
  if (raw !== null) return preferencesSchema.parse(JSON.parse(raw));
  return { schema_version: 1, shell: 'new', collapsed: legacySidebar === 'collapsed', start: '/', pins: [] };
}

export function navigationArea(pathname: string): string {
  if (pathname === '/') return 'Your work';
  if (pathname === '/account' || pathname.startsWith('/account/')) return 'Account';
  if (pathname.startsWith('/runs/')) return 'Observability';
  const section = sectionOf(pathname);
  return (section && areaOf(section, pathname)?.label) || 'Unknown page';
}

type Transition = { from: string; to: string; shell: ShellMode; count: number };
interface NavigationState {
  preferences: NavigationPreferences;
  shell: ShellMode;
  policy: ShellPolicy;
  available: boolean;
  message: string;
  update: (change: (previous: NavigationPreferences) => NavigationPreferences) => void;
  transitions: Transition[];
}
const NavigationContext = React.createContext<NavigationState | null>(null);

export function useNavigationPreferences(): NavigationState {
  const context = React.useContext(NavigationContext);
  if (!context) throw new Error('NavigationProvider is missing');
  return context;
}

export function NavigationProvider({ children, policy = shellPolicy(import.meta.env.VITE_AIWATCHER_SHELL) }: {
  children: React.ReactNode; policy?: ShellPolicy;
}) {
  const scope = useLocalScope();
  // Reset the entire local state at an identity/instance boundary, never render the previous user's pins.
  return <ScopedNavigationProvider key={scope ?? 'unavailable'} scope={scope} policy={policy}>{children}</ScopedNavigationProvider>;
}

function ScopedNavigationProvider({ children, scope, policy }: {
  children: React.ReactNode; scope?: string; policy: ShellPolicy;
}) {
  const key = scope ? `${scope}:navigation` : undefined;
  function read() {
    try {
      const raw = key ? window.localStorage.getItem(key) : null;
      const legacy = key && raw === null ? window.localStorage.getItem('aiwatcher.sidebar') : null;
      return { preferences: readNavigationPreferences(raw, legacy), message: '', unsaved: false };
    } catch {
      return { preferences: readNavigationPreferences(null), message: 'Navigation preferences could not be read. Stored data was left unchanged.', unsaved: true };
    }
  }
  const [state, setState] = React.useState(read);
  const current = React.useRef(state);
  const [transitions, setTransitions] = React.useState<Transition[]>([]);
  const shell = policy === 'user' ? state.preferences.shell : policy;
  const router = useRouter();

  React.useEffect(() => {
    const listener = (event: StorageEvent) => {
      if (event.key !== key && event.key !== null) return;
      const next = read();
      current.current = next;
      setState(next);
    };
    window.addEventListener('storage', listener);
    return () => window.removeEventListener('storage', listener);
  }, [key]);

  React.useEffect(() => router.subscribe('onResolved', ({ fromLocation, toLocation }) => {
    if (!fromLocation || fromLocation.href === toLocation.href) return;
    const from = navigationArea(fromLocation.pathname), to = navigationArea(toLocation.pathname);
    // Aggregate only area names after navigation completes: no object IDs, query text, or identity.
    if (from === to) return;
    setTransitions((previous) => {
      const existing = previous.find((entry) => entry.from === from && entry.to === to && entry.shell === shell);
      return existing
        ? previous.map((entry) => entry === existing ? { ...entry, count: entry.count + 1 } : entry)
        : [...previous.slice(-99), { from, to, shell, count: 1 }];
    });
  }), [router, shell]);

  function update(change: (previous: NavigationPreferences) => NavigationPreferences) {
    if (!key) return;
    let base = current.current.preferences;
    let readable = true;
    // Merge with the latest saved value so another tab's unrelated edit is retained.
    try {
      const latest = readNavigationPreferences(window.localStorage.getItem(key), window.localStorage.getItem('aiwatcher.sidebar'));
      if (!current.current.unsaved) base = latest;
    } catch { readable = false; }
    let preferences: NavigationPreferences;
    try { preferences = preferencesSchema.parse(change(base)); }
    catch {
      const next = { ...current.current, message: 'Could not apply this preference. Use an internal panel view, a name up to 80 characters and at most 20 pins.' };
      current.current = next;
      setState(next);
      return;
    }
    let message = 'Saved on this device.', unsaved = false;
    try {
      if (!readable) throw new Error('Preserve unreadable or newer schema');
      const serialized = JSON.stringify(preferences);
      window.localStorage.setItem(key, serialized);
      if (window.localStorage.getItem(key) !== serialized) throw new Error('Write was not retained');
    } catch {
      message = 'Applied for this session; local persistence could not be confirmed. Unreadable stored data is not replaced.';
      unsaved = true;
    }
    const next = { preferences, message, unsaved };
    current.current = next;
    setState(next);
  }

  return <NavigationContext.Provider value={{ preferences: state.preferences, shell, policy, available: !!scope, message: state.message, update, transitions }}>
    {children}
  </NavigationContext.Provider>;
}
