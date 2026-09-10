import * as React from 'react';
import { Link, Outlet, createRootRouteWithContext, useRouterState } from '@tanstack/react-router';
import type { QueryClient } from '@tanstack/react-query';
import { Activity, PanelLeftClose, PanelLeftOpen } from 'lucide-react';

import { UserMenu } from '@/components/user-menu';
import { SECTIONS, areaOf, sectionOf, type NavArea, type NavSection } from '@/lib/navigation';
import { cn } from '@/lib/utils';

/**
 * Three sections in the header, and the section's own areas down the side.
 *
 * The taxonomy itself is in `lib/navigation.ts` and the reasoning for it is
 * there; what this file owns is only how it is drawn. Two rules hold it:
 *
 * **The section comes from the URL, never from a click.** A run detail reached
 * from a link in Slack has to light up Inference the same way one reached by
 * pressing the tab does, so the header reads `sectionOf(pathname)` rather than
 * remembering which tab was last pressed. A path in no section — there are
 * none today, and a new area is one commit away from being one — lights up
 * nothing rather than defaulting to the first, because a wrong highlight is
 * worse than an absent one.
 *
 * **The sidebar collapses and the choice is remembered.** The annotation
 * canvas and the pipeline canvas both want the width, and somebody who
 * collapsed it for one of them did not mean "for this page load". It is the
 * one thing in this panel kept in `localStorage` rather than in the URL: it is
 * a per-viewer convenience and belongs to the reader, not to the link they
 * would send.
 */

export const Route = createRootRouteWithContext<{ queryClient: QueryClient }>()({
  component: RootLayout,
  notFoundComponent: () => (
    <div className="p-10 text-center text-sm text-muted-foreground">
      No such page.{' '}
      <Link to="/observability/explore" className="text-primary underline">
        Back to the explorer
      </Link>
      .
    </div>
  ),
});

const SIDEBAR_KEY = 'aiwatcher.sidebar';

function useCollapsed(): [boolean, () => void] {
  const [collapsed, setCollapsed] = React.useState(() => {
    // Every read and write is guarded: a private window, a browser set to
    // block site data and a thumbnail capture all throw on the accessor
    // itself, and a navigation that will not render is worse than a sidebar
    // that forgot.
    try {
      return localStorage.getItem(SIDEBAR_KEY) === 'collapsed';
    } catch {
      return false;
    }
  });

  const toggle = React.useCallback(() => {
    setCollapsed((value) => {
      try {
        localStorage.setItem(SIDEBAR_KEY, value ? 'open' : 'collapsed');
      } catch {
        /* The preference is not worth failing a click over. */
      }
      return !value;
    });
  }, []);

  return [collapsed, toggle];
}

function RootLayout() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const section = sectionOf(pathname);
  const [collapsed, toggle] = useCollapsed();

  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-20 border-b border-border bg-background/80 backdrop-blur">
        <div className="flex items-center gap-4 px-4">
          <Link
            to="/observability/explore"
            className="flex shrink-0 items-center gap-2 py-3 font-semibold"
          >
            <Activity className="h-4 w-4 text-primary" />
            aiwatcher
          </Link>

          {/* Three, so they fit at any width and never need to scroll — which
              is what the eleven-area row could not manage. */}
          <nav className="flex min-w-0 items-center gap-1">
            {SECTIONS.map((candidate) => (
              <Link
                key={candidate.id}
                to={candidate.home}
                aria-current={section?.id === candidate.id ? 'page' : undefined}
                title={candidate.blurb}
                className={cn(
                  'flex shrink-0 items-center gap-1.5 whitespace-nowrap border-b-2 px-3 py-3 text-sm transition-colors hover:text-foreground',
                  section?.id === candidate.id
                    ? 'border-primary text-foreground'
                    : 'border-transparent text-muted-foreground',
                )}
              >
                <candidate.icon className="h-3.5 w-3.5" />
                {candidate.label}
              </Link>
            ))}
          </nav>

          <div className="ml-auto flex items-center gap-2">
            <UserMenu />
          </div>
        </div>
      </header>

      <div className="flex">
        {section ? (
          <SectionSidebar
            section={section}
            pathname={pathname}
            collapsed={collapsed}
            onToggle={toggle}
          />
        ) : null}
        <main className="min-w-0 flex-1 px-4 py-4 md:px-6 md:py-6">
          <div className="mx-auto flex max-w-[100rem] flex-col gap-4">
            {section ? <NarrowNav section={section} pathname={pathname} /> : null}
            <Outlet />
          </div>
        </main>
      </div>
    </div>
  );
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
}: {
  section: NavSection;
  pathname: string;
  collapsed: boolean;
  onToggle: () => void;
}) {
  const active = areaOf(section, pathname);

  return (
    <aside
      className={cn(
        'sticky top-[3.25rem] hidden h-[calc(100vh-3.25rem)] shrink-0 border-r border-border transition-[width] md:block',
        collapsed ? 'w-14' : 'w-56',
      )}
    >
      <div className="flex h-full flex-col overflow-y-auto py-3">
        <nav className="flex flex-1 flex-col gap-0.5 px-2">
          {section.areas.map((area) => {
            const isActive = active?.to === area.to;
            return (
              <div key={area.to} className="flex flex-col">
                <Link
                  to={area.to}
                  title={collapsed ? area.label : area.blurb}
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
function NarrowNav({ section, pathname }: { section: NavSection; pathname: string }) {
  const active = areaOf(section, pathname);

  return (
    <div className="flex flex-col gap-1 md:hidden">
      <nav className="-mx-4 flex items-center gap-1 overflow-x-auto px-4">
        {section.areas.map((area) => (
          <Link
            key={area.to}
            to={area.to}
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
