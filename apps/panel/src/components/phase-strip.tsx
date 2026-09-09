import * as React from 'react';
import { ChevronRight, Layers, Maximize2 } from 'lucide-react';

import type { Phase, PhaseId } from '@/lib/phases';
import { cn, formatCount } from '@/lib/utils';

/**
 * The curation at the level most questions are asked at.
 *
 * Ten boxes and nine arrows is a chain somebody traces with a finger. The same
 * ten as *Ingest → Data engineering → ML → Output* is a shape read in a
 * second, and it survives a canvas whose blocks somebody has dragged into two
 * rows — which a band drawn around them would not.
 *
 * Clicking one selects that stretch of the chain: its blocks stay lit and the
 * rest recede, through the same `away` the reach trace uses. The icon beside
 * the count folds the phase into a single box on the canvas instead, which is
 * what makes a ten-block chain navigable — six data-engineering steps become
 * one, and the four boxes left fit at full size.
 *
 * Both are *reading* controls. Nothing here edits, saves or is saved, which is
 * the whole difference between folding a phase and tidying the layout.
 */

const TONE: Record<PhaseId, { line: string; tint: string; dot: string }> = {
  ingest: { line: 'border-flow-data/70', tint: 'bg-flow-data/10', dot: 'bg-flow-data' },
  engineering: {
    line: 'border-flow-compute/70',
    tint: 'bg-flow-compute/10',
    dot: 'bg-flow-compute',
  },
  ml: { line: 'border-flow-runtime/70', tint: 'bg-flow-runtime/10', dot: 'bg-flow-runtime' },
  gate: { line: 'border-flow-gate/70', tint: 'bg-flow-gate/10', dot: 'bg-flow-gate' },
  output: { line: 'border-flow-publish/70', tint: 'bg-flow-publish/10', dot: 'bg-flow-publish' },
};

export function PhaseStrip({
  phases,
  selected,
  onSelect,
  collapsed,
  onToggleCollapsed,
  className,
}: {
  phases: Phase[];
  selected?: PhaseId | undefined;
  onSelect: (phase: PhaseId | undefined) => void;
  collapsed: ReadonlySet<PhaseId>;
  onToggleCollapsed: (phase: PhaseId) => void;
  className?: string;
}) {
  if (phases.length === 0) return null;
  return (
    <div className={cn('flex flex-wrap items-center gap-1', className)}>
      {phases.map((phase, index) => {
        const tone = TONE[phase.id];
        const active = selected === phase.id;
        const shut = collapsed.has(phase.id);
        return (
          <React.Fragment key={phase.id}>
            {index > 0 ? (
              <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground/50" />
            ) : null}
            <span
              className={cn(
                'flex items-center rounded-md border transition-colors',
                tone.line,
                active ? tone.tint : 'bg-card',
              )}
            >
              <button
                type="button"
                aria-pressed={active}
                title={`${phase.hint} — click to isolate this stretch`}
                onClick={() => onSelect(active ? undefined : phase.id)}
                className="flex items-center gap-2 rounded-l-md py-1.5 pl-2.5 pr-2 text-left transition-colors hover:bg-accent"
              >
                <span className={cn('h-2 w-2 shrink-0 rounded-[3px]', tone.dot)} />
                <span className="text-sm font-medium text-foreground">{phase.label}</span>
                <span className="tabular-nums text-[0.7rem] text-muted-foreground">
                  {formatCount(phase.blocks.length)}
                </span>
              </button>
              <button
                type="button"
                aria-pressed={shut}
                title={shut ? 'Show these blocks again' : 'Fold these blocks into one box'}
                onClick={() => onToggleCollapsed(phase.id)}
                className="rounded-r-md border-l border-inherit px-1.5 py-2 text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              >
                {shut ? <Maximize2 className="h-3 w-3" /> : <Layers className="h-3 w-3" />}
              </button>
            </span>
          </React.Fragment>
        );
      })}
    </div>
  );
}
