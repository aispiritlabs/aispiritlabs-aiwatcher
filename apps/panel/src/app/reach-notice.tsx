import { useRouterState } from '@tanstack/react-router';
import { Globe, Layers } from 'lucide-react';

import { reachOf } from '@/app/navigation';
import { useAuthConfig } from '@/shared/lib/auth';
import { useGrantedProjects, useOrganizations } from '@/shared/lib/iam';
import { parseScope } from '@/shared/lib/scope';
import { cn } from '@/shared/lib/utils';

/**
 * One line saying which side of the project boundary this page is answering on.
 *
 * The selector in the header says which project is selected. This says how far
 * that selection actually reached, and it exists because the honest answer is
 * not the same on every screen. Two misreadings it prevents, and they are
 * opposite:
 *
 * * with a project selected, an area the data plane has not scoped still
 *   answers for the whole deployment — **including other projects' work** —
 *   and nothing on the screen would otherwise say so. Those folds carry no
 *   project in the row, so the panel cannot mark the rows either: the same
 *   fact that keeps the area instance-wide is what stops a badge on each row.
 * * with none selected, the runs, spans and metrics on screen are the
 *   **unassigned** ones rather than all of them. Before M1 those two were the
 *   same set; they are not any more, and somebody whose project's runs stopped
 *   appearing deserves the sentence rather than a bug report.
 *
 * Shown only where the question arises: an instance whose caller is in no
 * organization has one side and needs no line about it.
 */
export function ReachNotice() {
  const config = useAuthConfig();
  const signedIn = config.data?.enabled === true;
  const organizations = useOrganizations(signedIn);
  const { projects } = useGrantedProjects();
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const raw = useRouterState({
    select: (state) => (state.location.search as { scope?: string }).scope,
  });

  const reach = reachOf(pathname);
  const selected = parseScope(raw);
  if (!signedIn || !reach) return null;
  if (!selected && projects.length === 0) return null;
  if (!selected && (organizations.data?.length ?? 0) === 0) return null;

  if (selected) {
    if (reach === 'project') return null;
    return (
      <Line
        tone="warn"
        icon={<Layers className="h-3.5 w-3.5 shrink-0" />}
        text={
          reach === 'instance'
            ? 'Instance-wide: nothing on this page is narrowed to the selected project. It answers for the whole deployment, other projects’ work included.'
            : 'Partly instance-wide: some of this page is the selected project’s and some of it answers for the whole deployment, other projects’ work included.'
        }
      />
    );
  }

  if (reach === 'instance') return null;
  return (
    <Line
      tone="quiet"
      icon={<Globe className="h-3.5 w-3.5 shrink-0" />}
      text="Unassigned: what has no project. A project’s own runs, spans and metrics answer under its own name, not here."
    />
  );
}

function Line({
  tone,
  icon,
  text,
}: {
  tone: 'warn' | 'quiet';
  icon: React.ReactNode;
  text: string;
}) {
  return (
    <p
      className={cn(
        'flex items-start gap-2 rounded border px-3 py-1.5 text-xs',
        tone === 'warn'
          ? 'border-amber-500/40 bg-amber-500/5 text-amber-700 dark:text-amber-400'
          : 'border-border bg-muted/40 text-muted-foreground',
      )}
    >
      {icon}
      <span>{text}</span>
    </p>
  );
}
