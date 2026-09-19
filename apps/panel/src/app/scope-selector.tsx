import * as React from 'react';
import { useNavigate, useRouterState } from '@tanstack/react-router';
import { Boxes, Check, Globe } from 'lucide-react';

import { landingFor } from '@/app/navigation';
import { useAuthConfig } from '@/shared/lib/auth';
import { useGrantedProjects, useOrganizations } from '@/shared/lib/iam';
import { formatScope, parseScope } from '@/shared/lib/scope';
import { Badge } from '@/shared/components/ui/primitives';
import { cn } from '@/shared/lib/utils';

/**
 * Which project the panel is reading, in the header.
 *
 * For a year this control was refused by name: a switcher scoping the whole
 * panel would announce a multi-tenancy the data plane could not keep. M1 is
 * what changed — runs, spans, dimensions, metrics and the live stream now
 * answer one project and refuse the rest, in both directions, and a revoked
 * grant closes a stream somebody already had open. What has **not** changed is
 * that the boundary stops short of some areas, so this control does not claim
 * the whole panel: it says what it reaches, and `reach-notice.tsx` says where
 * it does not.
 *
 * Three states, and the third is the one worth naming. A caller in no
 * organization sees nothing at all — an instance that never used IAM is a
 * single-tenant instance and gains no control it cannot use. A caller with
 * grants picks between them and the unassigned side. A caller who is in an
 * organization and holds no grant sees the control saying exactly that, rather
 * than an empty menu that reads like a page that failed to load.
 */
export function ScopeSelector() {
  const config = useAuthConfig();
  const signedIn = config.data?.enabled === true;
  const organizations = useOrganizations(signedIn);
  const { projects, settled } = useGrantedProjects();
  const navigate = useNavigate();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const raw = useRouterState({
    select: (state) => (state.location.search as { scope?: string }).scope,
  });
  const [open, setOpen] = React.useState(false);
  const trigger = React.useRef<HTMLButtonElement>(null);
  const panelId = React.useId();

  const selected = parseScope(raw);
  const chosen = selected
    ? projects.find(
        (entry) =>
          entry.organization.id === selected.organization &&
          entry.access.project.scope.project === selected.project,
      )
    : undefined;

  // Nothing to choose between, and nothing to explain: no sign-in, no IAM on
  // this deployment, or a caller in no organization at all.
  const inOrganizations = (organizations.data?.length ?? 0) > 0;
  if (!signedIn || organizations.isError || (!inOrganizations && !selected)) return null;

  const pick = (next: string | undefined) => {
    setOpen(false);
    void navigate({ to: landingFor(pathname), search: { scope: next } });
  };

  const label = chosen ? chosen.access.project.name : selected ? 'No access' : 'Unassigned';

  return (
    <div
      className="relative min-w-0"
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          setOpen(false);
          trigger.current?.focus();
        }
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <button
        type="button"
        ref={trigger}
        onClick={() => setOpen((was) => !was)}
        aria-controls={panelId}
        aria-expanded={open}
        className={cn(
          'flex items-center gap-2 rounded-md border border-border px-2 py-1 text-xs transition-colors hover:bg-accent',
          selected && !chosen ? 'border-destructive/60 text-destructive' : 'text-muted-foreground',
        )}
      >
        {selected ? <Boxes className="h-3.5 w-3.5" /> : <Globe className="h-3.5 w-3.5" />}
        <span className="max-w-[7rem] truncate text-foreground sm:max-w-[10rem]">{label}</span>
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
          <div
            id={panelId}
            role="region"
            aria-label="Project being read"
            // Scrolls rather than runs off the bottom: an instructor with a
            // term's worth of workshops has one grant per project, and the
            // list is as long as their year.
            className="absolute right-0 z-20 mt-1 max-h-[70vh] w-80 overflow-y-auto rounded-md border border-border bg-card p-2 shadow-lg"
          >
            <Choice
              chosen={!selected}
              title="Unassigned"
              note="Runs, spans and metrics that belong to no project. Not the whole instance — a project's own are under its own name."
              onClick={() => pick(undefined)}
            />

            {projects.length > 0 && (
              <div className="mt-1 border-t border-border pt-1">
                {projects.map(({ organization, access }) => {
                  const scope = {
                    organization: organization.id,
                    project: access.project.scope.project,
                  };
                  return (
                    <Choice
                      key={formatScope(scope)}
                      chosen={
                        selected?.organization === scope.organization &&
                        selected.project === scope.project
                      }
                      title={access.project.name}
                      note={organization.name}
                      badge={access.role}
                      onClick={() => pick(formatScope(scope))}
                    />
                  );
                })}
              </div>
            )}

            {settled && projects.length === 0 && (
              <p className="border-t border-border px-2 py-2 text-xs text-muted-foreground">
                You hold no project grant. Somebody who administers a project can invite you to one;
                until then this panel reads the unassigned side.
              </p>
            )}

            {selected && !chosen && settled && (
              <p className="border-t border-border px-2 py-2 text-xs text-destructive">
                This link names a project you hold no live grant on. Its runs answer as though they
                were not there, which is the same answer somebody gets for a project that never
                existed.
              </p>
            )}
          </div>
        </>
      )}
    </div>
  );
}

function Choice({
  chosen,
  title,
  note,
  badge,
  onClick,
}: {
  chosen: boolean;
  title: string;
  note: string;
  badge?: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={chosen ? 'true' : undefined}
      className="flex w-full items-start gap-2 rounded px-2 py-2 text-left hover:bg-accent"
    >
      <Check className={cn('mt-0.5 h-3.5 w-3.5 shrink-0', chosen ? 'text-primary' : 'opacity-0')} />
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2">
          <span className="truncate text-sm">{title}</span>
          {badge && <Badge tone="neutral">{badge}</Badge>}
        </span>
        <span className="mt-0.5 block text-xs text-muted-foreground">{note}</span>
      </span>
    </button>
  );
}
