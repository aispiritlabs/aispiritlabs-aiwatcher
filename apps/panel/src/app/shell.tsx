import * as React from 'react';
import { Link, Outlet, useRouterState } from '@tanstack/react-router';
import { Activity, PanelLeftClose, PanelLeftOpen, Search } from 'lucide-react';

import { Appearance } from '@/shared/components/appearance';
import { UserMenu } from '@/shared/components/user-menu';
import { CommandPanel, useCommandPanel } from '@/app/command-panel';
import { SECTIONS, areaOf, sectionOf, type NavArea, type NavSection } from '@/app/navigation';
import { cn } from '@/shared/lib/utils';
import { NavigationProvider, useNavigationPreferences } from '@/app/navigation-preferences';
import { NavigationMessage, PinCurrentView, PinnedViews } from '@/app/navigation-controls';

/** Global identity and search above peer work areas. Active links follow the URL. */

export function RootLayout() {
  return <NavigationProvider><ShellLayout /></NavigationProvider>;
}

/** The account area is not in the navigation, so its pages name themselves. */
function accountTitle(pathname: string): string | undefined {
  if (pathname === '/account') return 'Profile & account';
  if (pathname === '/account/access') return 'Organizations & projects';
  return undefined;
}

function ShellLayout() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const matchedSection = sectionOf(pathname);
  const section = matchedSection ?? SECTIONS[0];
  React.useEffect(() => {
    const area = matchedSection && areaOf(matchedSection, pathname);
    const page = area && (area.views.find((view) => view.to === pathname)?.label ?? area.label);
    document.title = `${pathname === '/' ? 'Your work' : accountTitle(pathname) ?? page ?? 'Page'} · aiwatcher`;
  }, [pathname, matchedSection]);
  const { preferences, shell, update } = useNavigationPreferences();
  const collapsed = preferences.collapsed;
  const toggle = () => update((previous) => ({ ...previous, collapsed: !previous.collapsed }));
  const [commandsOpen, setCommandsOpen] = useCommandPanel();

  return (
    <div className="min-h-screen">
      <CommandPanel open={commandsOpen} onOpenChange={setCommandsOpen} />
      <header className="sticky top-0 z-20 border-b border-border bg-background/80 backdrop-blur">
        <div className="flex items-center gap-4 px-4">
          <Link
            to="/"
            search={{ start: 'workspace' }}
            className="flex shrink-0 items-center gap-2 py-3 font-semibold"
          >
            <Activity className="h-4 w-4 text-primary" />
            aiwatcher
          </Link>

          <div className="ml-auto flex items-center gap-2">
            {/* Visible as well as bound to a shortcut: a palette nobody is
                told about is a palette nobody opens, and the hint is where
                the shortcut gets learnt. */}
            <button
              type="button"
              onClick={() => setCommandsOpen(true)}
              aria-label="Search or jump to a page"
              className="flex items-center gap-2 rounded border border-border px-2 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground sm:flex"
            >
              <Search className="h-3 w-3" />
              <span className="hidden sm:inline">Search or jump to…</span>
              <kbd className="hidden sm:inline rounded border border-border px-1 font-mono text-[10px]">⌘K</kbd>
            </button>
            <Appearance />
            <UserMenu />
          </div>
        </div>
      </header>

      {shell === 'classic' && <nav aria-label="Work areas" className="flex gap-1 overflow-x-auto border-b border-border px-4">
        {SECTIONS.map((group) => <Link key={group.id} to={group.home} onFocus={reveal} aria-current={matchedSection?.id === group.id ? 'page' : undefined}
          className={cn('shrink-0 border-b-2 px-3 py-3 text-sm', matchedSection?.id === group.id ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground')}>
          {group.label}
        </Link>)}
      </nav>}

      <div className="flex">
        {section ? (
          <SectionSidebar
            section={section}
            pathname={pathname}
            collapsed={collapsed}
            onToggle={toggle}
            groups={shell === 'classic' ? [section] : SECTIONS}
          />
        ) : null}
        <main className="min-w-0 flex-1 px-4 py-4 md:px-6 md:py-6">
          <div className="mx-auto flex max-w-[100rem] flex-col gap-4">
            {matchedSection ? <NarrowNav section={matchedSection} pathname={pathname} showGroups={shell !== 'classic'} /> : null}
            <PinCurrentView />
            <NavigationMessage />
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  );
}

/**
 * Keyboard focus has to land somewhere the reader can see.
 *
 * Every navigation row that can outgrow a phone is an `overflow-x-auto`
 * scroller, and Chrome does **not** reliably scroll a focused link inside one
 * into view: tabbing through Data's areas at 375 px put focus on Conversations
 * at x=373..505 with the row still at scrollLeft 0, so the focus ring was
 * entirely off the right edge and the only clue to where focus had gone was a
 * two-pixel sliver. `inline: 'nearest'` moves the row the minimum it takes, and
 * `block: 'nearest'` keeps it from scrolling the page as a side effect.
 */
function reveal(event: React.FocusEvent<HTMLElement>) {
  event.currentTarget.scrollIntoView({ block: 'nearest', inline: 'nearest' });
}

/**
 * Keep one search parameter across a move between an area's views, and drop
 * the rest.
 *
 * Returning `{}` rather than the previous search is the point: the pivot, the
 * open run and a half-written query all belong to the view they were set in,
 * and carrying them into the next one lands the reader on a filtered page they
 * did not ask for.
 */
function carry(
  parameter: NavArea['carries'],
): (previous: Record<string, unknown>) => Record<string, unknown> {
  return (previous) => {
    if (!parameter) return {};
    const value = previous[parameter];
    return value === undefined ? {} : { [parameter]: value };
  };
}

/**
 * The section's areas, each with its own pages under it.
 *
 * Both levels are visible at once rather than one being a disclosure: the
 * whole point of the change is that the page you want should be reachable
 * without first guessing which area holds it.
 */
function SectionSidebar({
  section,
  pathname,
  collapsed,
  onToggle,
  groups,
}: {
  section: NavSection;
  pathname: string;
  collapsed: boolean;
  onToggle: () => void;
  groups: NavSection[];
}) {
  return (
    <aside
      className={cn(
        'sticky top-[3.25rem] hidden h-[calc(100vh-3.25rem)] shrink-0 border-r border-border transition-[width] md:block',
        collapsed ? 'w-14' : 'w-56',
      )}
    >
      <div className="flex h-full flex-col overflow-y-auto py-3">
        <nav aria-label="Main navigation" className="flex flex-1 flex-col gap-0.5 px-2">
          <Link to="/" search={{ start: 'workspace' }} title="Your work" aria-label="Your work" aria-current={pathname === '/' ? 'page' : undefined} className="mb-3 flex items-center gap-2 rounded px-2 py-2 text-sm hover:bg-accent"><Activity className="h-4 w-4" />{!collapsed && 'Your work'}</Link>
          {!collapsed && <PinnedViews compact />}
          {groups.map((group) => (
            <div key={group.id} className="mb-3">
              {!collapsed && <p className="px-2 py-1 text-xs font-medium text-muted-foreground">{group.label}</p>}
              {group.areas.map((area) => {
            const isActive = section.id === group.id && areaOf(group, pathname)?.to === area.to;
            return (
              <div key={area.to} className="flex flex-col">
                <Link
                  to={area.to}
                  title={collapsed ? area.label : area.blurb}
                  aria-label={area.label}
                  aria-current={isActive ? 'page' : undefined}
                  className={cn(
                    'flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors',
                    isActive
                      ? 'bg-accent font-medium text-foreground'
                      : 'text-muted-foreground hover:bg-accent/50 hover:text-foreground',
                  )}
                >
                  <area.icon className="h-4 w-4 shrink-0" />
                  {collapsed ? null : <span className="truncate">{area.label}</span>}
                </Link>

                {/* Only the area you are in expands. Every view of every area
                    at once is the flat list again, one level down. */}
                {collapsed || !isActive || area.views.length === 0 ? null : (
                  <div className="ml-[1.4rem] mt-0.5 flex flex-col border-l border-border pl-2">
                    {area.views.map((view) => (
                      <Link
                        key={view.to}
                        to={view.to}
                        // What survives a move between an area's views is the
                        // area's own business — see `NavArea.carries`. This
                        // used to be one hard-coded `window`, which was right
                        // for Observability and silently dropped the project
                        // every time somebody moved from Label to Exports.
                        search={carry(area.carries)}
                        className="rounded-md px-2 py-1 text-[13px] text-muted-foreground transition-colors hover:text-foreground [&.active]:font-medium [&.active]:text-foreground"
                      >
                        {view.label}
                      </Link>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
            </div>
          ))}
        </nav>

        <button
          type="button"
          onClick={onToggle}
          aria-label={collapsed ? 'Expand the sidebar' : 'Collapse the sidebar'}
          className="mx-2 mt-2 flex items-center gap-2 rounded-md px-2 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
        >
          {collapsed ? (
            <PanelLeftOpen className="h-4 w-4 shrink-0" />
          ) : (
            <>
              <PanelLeftClose className="h-4 w-4 shrink-0" />
              Collapse
            </>
          )}
        </button>
      </div>
    </aside>
  );
}

/**
 * The same two levels, for a screen too narrow for the sidebar.
 *
 * Not a hamburger. The sidebar is hidden below `md` and something has to take
 * its place, or a phone gets a header with three sections and no way into any
 * of their pages — which is worse than the scrolling row this replaced. Two
 * scrolling rows, because that is what the sidebar is: the section's areas,
 * and then the pages of whichever one you are in.
 */
function NarrowNav({ section, pathname, showGroups }: { section: NavSection; pathname: string; showGroups: boolean }) {
  const active = areaOf(section, pathname);

  return (
    <div className="flex flex-col gap-1 md:hidden">
      {showGroups && <nav aria-label="Work areas" className="flex gap-1 overflow-x-auto">
        {SECTIONS.map((group) => <Link key={group.id} to={group.home} onFocus={reveal} aria-current={section.id === group.id ? 'page' : undefined} className="shrink-0 rounded px-2 py-2 text-sm [&.active]:bg-accent">{group.label}</Link>)}
      </nav>}
      <nav className="-mx-4 flex items-center gap-1 overflow-x-auto px-4">
        {section.areas.map((area) => (
          <Link
            key={area.to}
            to={area.to}
            onFocus={reveal}
            className={cn(
              'flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-md px-2.5 py-1.5 text-sm transition-colors',
              active?.to === area.to
                ? 'bg-accent font-medium text-foreground'
                : 'text-muted-foreground',
            )}
          >
            <area.icon className="h-3.5 w-3.5" />
            {area.label}
          </Link>
        ))}
      </nav>
      {active && active.views.length > 0 ? (
        <nav className="-mx-4 flex items-center gap-1 overflow-x-auto border-b border-border px-4">
          {active.views.map((view) => (
            <Link
              key={view.to}
              to={view.to}
              onFocus={reveal}
              search={carry(active.carries)}
              className="-mb-px shrink-0 whitespace-nowrap border-b-2 border-transparent px-2.5 py-1.5 text-[13px] text-muted-foreground transition-colors [&.active]:border-primary [&.active]:text-foreground"
            >
              {view.label}
            </Link>
          ))}
        </nav>
      ) : null}
    </div>
  );
}
