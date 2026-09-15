import * as React from 'react';
import { Link, useRouter, useRouterState } from '@tanstack/react-router';
import { Pin, X } from 'lucide-react';
import { START_PAGES, isPanelHref, navigationArea, useNavigationPreferences } from '@/app/navigation-preferences';
import { Button } from '@/shared/components/ui/primitives';

/** Used only on the index page. An explicit workspace link always bypasses the preferred start. */
export function PreferredStart({ children }: { children: React.ReactNode }) {
  const { preferences } = useNavigationPreferences();
  const router = useRouter();
  // Capture once: changing the preference while working on the index must not navigate away.
  const [target] = React.useState(() => router.state.location.searchStr || router.state.location.hash ? '/' : preferences.start);
  const redirected = React.useRef(false);
  React.useEffect(() => {
    if (target === '/' || redirected.current) return;
    redirected.current = true;
    void router.navigate({ to: target, search: {}, replace: true });
  }, [router, target]);
  return target === '/' ? children : <p role="status">Opening your start page…</p>;
}

function PinnedLink({ href, children, className }: { href: string; children: React.ReactNode; className?: string }) {
  const router = useRouter();
  const url = new URL(href, window.location.origin);
  return <Link to={url.pathname} search={router.options.parseSearch(url.search)} hash={url.hash.slice(1)} className={className} title={href}>
    {children}
  </Link>;
}

export function PinnedViews({ compact = false }: { compact?: boolean }) {
  const { preferences, update } = useNavigationPreferences();
  if (compact && preferences.pins.length === 0) return null;
  return <section aria-label="Pinned views" className={compact ? 'mb-3 border-b border-border pb-3' : 'rounded-lg border border-border p-4'}>
    <h2 className="mb-2 text-sm font-medium">Pinned views</h2>
    {preferences.pins.length === 0
      ? <p className="text-sm text-muted-foreground">Open a view and choose Pin this view to keep its filters and version on this device.</p>
      : <ul className="grid gap-1">
        {preferences.pins.map((pin) => <li key={pin.href} className="flex min-w-0 items-center gap-1">
          <PinnedLink href={pin.href} className="min-w-0 flex-1 rounded px-2 py-2 text-sm hover:bg-accent">
            <span className="block truncate">{pin.label}</span>
            {!compact && <span className="mt-1 block truncate text-xs text-muted-foreground">{pin.href}</span>}
          </PinnedLink>
          <button type="button" className="shrink-0 rounded p-2 text-muted-foreground hover:bg-accent hover:text-foreground"
            aria-label={`Unpin ${pin.label}`} onClick={() => update((previous) => ({ ...previous, pins: previous.pins.filter((entry) => entry.href !== pin.href) }))}>
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </li>)}
      </ul>}
  </section>;
}

export function PinCurrentView() {
  const location = useRouterState({ select: (state) => state.location });
  return <PinForm key={location.href} href={location.href} pathname={location.pathname} />;
}

function PinForm({ href, pathname }: { href: string; pathname: string }) {
  const { preferences, available, update } = useNavigationPreferences();
  const [label, setLabel] = React.useState(() => {
    const page = START_PAGES.find((page) => page.to === pathname);
    return (page?.label ?? `${navigationArea(pathname)} · ${pathname.split('/').at(-1)}`).slice(0, 80);
  });
  const pinned = preferences.pins.some((pin) => pin.href === href);
  if (pathname === '/' || !isPanelHref(href)) return null;
  return <div className="flex flex-wrap items-start justify-end gap-2 text-xs">
    <details className="max-w-full rounded border border-border p-2">
      <summary className="cursor-pointer">Navigation layout</summary>
      <div className="mt-3 max-w-64"><NavigationLayoutSelect /></div>
    </details>
    <Link to="/" search={{ start: 'workspace' }} hash="navigation-preferences" className="rounded px-2 py-2 text-muted-foreground hover:text-foreground">Navigation preferences</Link>
    {pinned ? <Button size="sm" variant="outline" onClick={() => update((previous) => ({ ...previous, pins: previous.pins.filter((pin) => pin.href !== href) }))}>
      <Pin className="mr-1 h-3 w-3" aria-hidden="true" />Unpin this view
    </Button> : <details className="max-w-full rounded border border-border p-2">
      <summary className="cursor-pointer">Pin this view</summary>
      <form className="mt-3 flex max-w-full flex-wrap items-end gap-2" onSubmit={(event) => {
        event.preventDefault();
        update((previous) => ({ ...previous, pins: [...previous.pins.filter((pin) => pin.href !== href), { href, label }] }));
      }}>
        <label className="grid min-w-0 gap-1">Pinned view name
          <input className="w-52 max-w-full rounded border border-border bg-background p-2" value={label} maxLength={80} onChange={(event) => setLabel(event.target.value)} />
        </label>
        <Button size="sm" type="submit" disabled={!available || !label.trim() || preferences.pins.length >= 20}>Pin view</Button>
        <p className="basis-full text-muted-foreground">Saves the current link, including filters and version. Unsaved edits and data are not saved.</p>
        {preferences.pins.length >= 20 && <p className="basis-full">20 pins saved. Remove one from Your work to add another.</p>}
      </form>
    </details>}
  </div>;
}

export function NavigationSettings() {
  const { preferences, available, update, transitions } = useNavigationPreferences();
  return <section id="navigation-preferences" aria-label="Navigation preferences" className="scroll-mt-20 rounded-lg border border-border p-4">
    <h2 className="text-sm font-medium">Navigation preferences</h2>
    <p className="mt-1 text-sm text-muted-foreground">Saved for this identity and instance on this device.</p>
    <div className="mt-4 grid gap-4 sm:grid-cols-2">
      <NavigationLayoutSelect />
      <label className="grid min-w-0 gap-2 text-sm">Start page
        <select aria-label="Start page" className="w-full rounded border border-border bg-background p-2" value={preferences.start} disabled={!available}
          onChange={(event) => update((previous) => ({ ...previous, start: event.target.value }))}>
          {START_PAGES.map((page) => <option key={page.to} value={page.to}>{page.label}</option>)}
        </select>
        <span className="text-xs text-muted-foreground">Used when opening the panel root. Shared links keep their destination; Your work always opens the overview.</span>
      </label>
    </div>
    <details className="mt-4 text-xs">
      <summary className="cursor-pointer text-muted-foreground">Navigation diagnostics · this session</summary>
      <p className="my-2 text-muted-foreground">Completed moves between work areas, grouped by layout. Kept in memory for this session; no IDs, filters or query text are collected or sent.</p>
      {transitions.length === 0 ? <p>No moves between areas recorded yet.</p> : <ul className="space-y-1">
        {transitions.map((entry) => <li key={`${entry.from}:${entry.to}:${entry.shell}`}>{entry.from} → {entry.to} · {entry.shell === 'new' ? 'All work areas' : 'Classic'}: {entry.count}</li>)}
      </ul>}
    </details>
  </section>;
}

function NavigationLayoutSelect() {
  const { shell, policy, available, update } = useNavigationPreferences();
  return <label className="grid min-w-0 gap-2 text-sm">Navigation layout
    <select aria-label="Navigation layout" className="w-full rounded border border-border bg-background p-2" value={shell} disabled={!available || policy !== 'user'}
      onChange={(event) => update((previous) => ({ ...previous, shell: event.target.value === 'classic' ? 'classic' : 'new' }))}>
      <option value="new">All work areas</option><option value="classic">Classic section navigation</option>
    </select>
    <span className="text-xs text-muted-foreground">{policy === 'user' ? 'Changes the menu while keeping your current page and edits.' : 'The navigation layout is fixed by this deployment.'}</span>
  </label>;
}

export function NavigationMessage() {
  const { available, message } = useNavigationPreferences();
  return <p role="status" className="text-xs text-muted-foreground">{available ? message : 'Navigation preferences are unavailable until identity is known.'}</p>;
}
