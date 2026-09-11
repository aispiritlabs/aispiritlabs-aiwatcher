import * as React from 'react';
import { ArrowLeftRight, ArrowRight, MoveHorizontal, X } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import type { ReachMode } from '@/shared/lib/reach';
import { cn } from '@/shared/lib/utils';

/**
 * The one visual vocabulary both graph canvases speak.
 *
 * There are two of them — a curation being edited and a workflow execution
 * being watched — and until this existed each drew its boxes from the status
 * palette alone. That made a canvas say how everything was *going* and never
 * what anything *was*: a source, a gate and a publish step were the same
 * colour until one of them failed, and the picture only became readable at the
 * moment something went wrong.
 *
 * So a box carries two facts at once. Its **role** is what it is, drawn as a
 * tinted fill and a coloured stroke, and it never changes. Its **state** is
 * what is happening to it, drawn as motion and a status line on top. The two
 * are independent because they answer different questions, and a reader
 * usually has the first one already and is looking for the second.
 *
 * The motion rule is the one worth keeping: **nothing loops except what is
 * happening right now.** An arrival glows once and is gone, a completed edge
 * is traced once as it completes, and only a step that is genuinely running
 * keeps a ring going — of which there are one or two, never a canvas. Motion
 * that repeats is motion that is ignored, and then resented. `styles.css`
 * holds the keyframes and turns all of it off under `prefers-reduced-motion`.
 */

/**
 * What a box is. A closed set of seven, and a new node kind picks the one it
 * belongs to rather than adding an eighth — a legend with a colour per kind is
 * a colour chart, and nobody reads one.
 */
export type FlowRole = 'data' | 'compute' | 'runtime' | 'gate' | 'publish' | 'agent' | 'neutral';

/** What is happening to it. The same four words both canvases already use. */
export type FlowState = 'idle' | 'live' | 'done' | 'failed';

/**
 * Written out rather than composed, because Tailwind reads source text: a
 * class assembled from `flow-${role}` at runtime is a class that never gets
 * generated, and the failure is a box with no colour rather than an error.
 */
const ROLE: Record<FlowRole, { line: string; tint: string; ink: string; swatch: string }> = {
  data: {
    line: 'border-flow-data/85',
    tint: 'bg-flow-data/14',
    ink: 'text-flow-data',
    swatch: 'bg-flow-data',
  },
  compute: {
    line: 'border-flow-compute/85',
    tint: 'bg-flow-compute/14',
    ink: 'text-flow-compute',
    swatch: 'bg-flow-compute',
  },
  runtime: {
    line: 'border-flow-runtime/85',
    tint: 'bg-flow-runtime/14',
    ink: 'text-flow-runtime',
    swatch: 'bg-flow-runtime',
  },
  gate: {
    line: 'border-flow-gate/85',
    tint: 'bg-flow-gate/14',
    ink: 'text-flow-gate',
    swatch: 'bg-flow-gate',
  },
  publish: {
    line: 'border-flow-publish/85',
    tint: 'bg-flow-publish/14',
    ink: 'text-flow-publish',
    swatch: 'bg-flow-publish',
  },
  agent: {
    line: 'border-flow-agent/85',
    tint: 'bg-flow-agent/14',
    ink: 'text-flow-agent',
    swatch: 'bg-flow-agent',
  },
  neutral: {
    line: 'border-flow-neutral/60',
    tint: 'bg-flow-neutral/10',
    ink: 'text-flow-neutral',
    swatch: 'bg-flow-neutral',
  },
};

export function roleInk(role: FlowRole): string {
  return ROLE[role].ink;
}

/**
 * One box.
 *
 * `children` is where the caller puts its handles: the two canvases connect
 * different sides for different reasons — a curation flows one way and agents
 * reply — and a component that decided that for them would be deciding
 * something neither of them agrees on.
 */
export function FlowCard({
  role,
  state,
  selected = false,
  provisional = false,
  dim = false,
  away = false,
  mark: Mark,
  title,
  sublabel,
  footer,
  className,
  children,
}: {
  role: FlowRole;
  state: FlowState;
  selected?: boolean;
  /**
   * A shape nothing declared. Drawn dashed rather than omitted, because the
   * graph having drifted from the code running it is the finding.
   */
  provisional?: boolean;
  /**
   * Recessive, because nothing has happened here yet.
   *
   * The caller decides, and the two canvases decide differently on purpose. A
   * declared stage nothing has started is genuinely recessive — that a stage
   * exists and has not run is the finding ADR_0012's declaration exists to
   * produce. A curation block nobody has pressed Run on is not: that is the
   * ordinary state of editing, and dimming it would say something is wrong
   * with a canvas somebody is halfway through building.
   */
  dim?: boolean;
  /**
   * Outside the reach somebody is tracing. Distinct from `dim` and much
   * stronger: `dim` is a fact about this node, `away` is a fact about the
   * question being asked of the whole canvas, and it is turned off again by
   * clearing the trace rather than by anything happening here.
   */
  away?: boolean;
  mark: LucideIcon;
  title: string;
  sublabel?: React.ReactNode;
  footer?: React.ReactNode;
  className?: string;
  children?: React.ReactNode;
}) {
  const tone = ROLE[role];
  return (
    <div
      className={cn(
        // A solid card *under* the role wash, not the wash alone. At 8% over a
        // near-black canvas a block read as a ghost of itself — the role has
        // to tint something before it can tint anything.
        'relative w-[13rem] rounded-lg border-2 bg-card px-3 py-2.5 shadow-sm transition-colors',
        tone.line,
        provisional && 'border-dashed',
        // One opacity, chosen here rather than composed from two classes.
        // `away` outranks `dim`: they are both true of a never-run block that
        // is off the traced path, and the question being asked of the canvas
        // wins over a fact about the block. Written as utilities because a
        // rule in `@layer components` loses to `opacity-60` — Tailwind orders
        // utilities after components, which is the same precedence trap the
        // edge rules hit with React Flow's unlayered stylesheet.
        away ? 'opacity-15 saturate-50' : dim ? 'opacity-60' : undefined,
        state === 'failed' && 'border-danger bg-danger/10',
        selected && 'ring-2 ring-primary ring-offset-2 ring-offset-background',
        className,
      )}
    >
      {/*
        The glow lives on an overlay rather than on the card, and is keyed by
        state so React remounts it and the animation replays for the change.
        Keying the card itself would remount the handles with it, and React
        Flow would lose the edges attached to them mid-render.
      */}
      <span
        aria-hidden
        className={cn('pointer-events-none absolute inset-0 rounded-md', tone.tint)}
      />
      <span
        key={state}
        aria-hidden
        className={cn(
          'pointer-events-none absolute inset-0 rounded-md',
          state === 'done' || state === 'failed' ? 'flow-arrive' : undefined,
          state === 'failed' ? 'text-danger' : tone.ink,
        )}
      />
      <div className="relative flex items-center gap-2">
        {/* The mark in a chip rather than loose beside the title: it is what
            makes a row of cards scannable by kind at a glance. */}
        <span
          className={cn(
            'flex h-5 w-5 shrink-0 items-center justify-center rounded-[5px] border',
            tone.line,
            tone.tint,
          )}
        >
          <Mark className={cn('h-3 w-3', tone.ink)} />
        </span>
        <span className="truncate text-sm font-medium text-foreground">{title}</span>
        {state === 'live' ? (
          <span
            className="ml-auto h-2 w-2 shrink-0 rounded-full bg-flow-live text-flow-live flow-live-ring"
            title="running"
          />
        ) : null}
      </div>
      {sublabel === undefined ? null : (
        <p className="relative mt-1 truncate pl-7 text-[0.7rem] text-muted-foreground">
          {sublabel}
        </p>
      )}
      {footer === undefined ? null : (
        <div className="relative mt-1 flex items-center gap-2.5 pl-7 text-[0.7rem] text-muted-foreground">
          {footer}
        </div>
      )}
      {children}
    </div>
  );
}

/**
 * How an edge is drawn, from the state of what it feeds.
 *
 * React Flow's own `animated` marches a dash forever whatever is going on, so
 * a finished chain and a running one look the same. These say which: idle is
 * recessive, live marches, done is traced once in the accent as it completes,
 * and failed is the only one that borrows the status palette.
 */
export function flowEdgeClass(state: FlowState, away = false): string {
  return away ? `flow-edge-${state} flow-edge-away` : `flow-edge-${state}`;
}

const MODES: { mode: ReachMode; icon: LucideIcon; hint: string }[] = [
  { mode: 'upstream', icon: ArrowRight, hint: 'What feeds it' },
  { mode: 'downstream', icon: ArrowRight, hint: 'What it feeds' },
  { mode: 'both', icon: ArrowLeftRight, hint: 'Both ways' },
];

/**
 * Trace what the selected node reaches.
 *
 * Only rendered with a selection, because there is nothing to trace without
 * one — an always-present control whose buttons do nothing most of the time
 * reads as broken. Off is the default and clearing the selection clears the
 * trace, so the canvas never keeps a filter somebody cannot see the cause of.
 */
export function ReachControl({
  mode,
  onChange,
  subject,
}: {
  mode: ReachMode | undefined;
  onChange: (mode: ReachMode | undefined) => void;
  subject: string;
}) {
  return (
    <div className="flex items-center gap-1 rounded-md border border-border bg-card/90 px-1.5 py-1 text-[0.7rem] shadow-sm backdrop-blur">
      <MoveHorizontal className="h-3 w-3 shrink-0 text-muted-foreground" />
      <span className="mr-0.5 max-w-[9rem] truncate text-muted-foreground" title={subject}>
        {subject}
      </span>
      {MODES.map(({ mode: candidate, icon: Icon, hint }) => (
        <button
          key={candidate}
          type="button"
          title={hint}
          aria-pressed={mode === candidate}
          onClick={() => onChange(mode === candidate ? undefined : candidate)}
          className={cn(
            'rounded px-1.5 py-0.5 transition-colors',
            mode === candidate
              ? 'bg-primary text-primary-foreground'
              : 'text-muted-foreground hover:bg-accent',
          )}
        >
          <Icon className={cn('h-3 w-3', candidate === 'upstream' && 'rotate-180')} />
        </button>
      ))}
      {mode ? (
        <button
          type="button"
          title="Stop tracing"
          onClick={() => onChange(undefined)}
          className="rounded px-1 py-0.5 text-muted-foreground transition-colors hover:bg-accent"
        >
          <X className="h-3 w-3" />
        </button>
      ) : null}
    </div>
  );
}

/** The roles on this canvas, in the domain's own words. */
export function FlowLegend({
  entries,
  className,
}: {
  entries: { role: FlowRole; label: string }[];
  className?: string;
}) {
  if (entries.length === 0) return null;
  return (
    <div
      className={cn(
        'flex flex-wrap items-center gap-x-4 gap-y-1.5 text-[0.7rem] text-muted-foreground',
        className,
      )}
    >
      {entries.map(({ role, label }) => (
        <span key={role} className="flex items-center gap-1.5">
          <span className={cn('h-2 w-2 rounded-[3px]', ROLE[role].swatch)} />
          {label}
        </span>
      ))}
    </div>
  );
}
