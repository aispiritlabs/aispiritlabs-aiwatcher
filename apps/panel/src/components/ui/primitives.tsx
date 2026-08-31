import * as React from 'react';
import { cva, type VariantProps } from 'class-variance-authority';
import { cn } from '@/lib/utils';

/**
 * The shadcn/ui primitives this panel actually uses, in one file.
 *
 * shadcn is a copy-in library rather than a dependency, so these live in the
 * repo by design. Kept together because there are six of them; split them out
 * when there are twenty.
 */

const buttonVariants = cva(
  'inline-flex items-center justify-center gap-2 rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary disabled:pointer-events-none disabled:opacity-50',
  {
    variants: {
      variant: {
        default: 'bg-primary text-primary-foreground hover:opacity-90',
        outline: 'border border-border bg-transparent hover:bg-accent',
        ghost: 'hover:bg-accent hover:text-accent-foreground',
      },
      size: {
        default: 'h-9 px-4 py-2',
        sm: 'h-8 rounded-md px-3 text-xs',
        icon: 'h-9 w-9',
      },
    },
    defaultVariants: { variant: 'default', size: 'default' },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {}

export function Button({ className, variant, size, ...props }: ButtonProps) {
  return <button className={cn(buttonVariants({ variant, size }), className)} {...props} />;
}

const badgeVariants = cva(
  'inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-medium',
  {
    variants: {
      tone: {
        neutral: 'border-border bg-muted text-muted-foreground',
        success: 'border-transparent bg-success/15 text-success',
        danger: 'border-transparent bg-danger/15 text-danger',
        running: 'border-transparent bg-running/15 text-running',
        warning: 'border-transparent bg-warning/15 text-warning',
        // The caller's own role, which is the one badge here that is about a
        // person rather than about a run.
        primary: 'border-transparent bg-primary/15 text-primary',
      },
    },
    defaultVariants: { tone: 'neutral' },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badgeVariants> {}

export function Badge({ className, tone, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ tone }), className)} {...props} />;
}

export function Card({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cn('rounded-lg border border-border bg-card text-card-foreground', className)}
      {...props}
    />
  );
}

export function CardHeader({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('flex flex-col gap-1 p-4', className)} {...props} />;
}

export function CardTitle({ className, ...props }: React.HTMLAttributes<HTMLHeadingElement>) {
  return <h3 className={cn('text-sm font-semibold tracking-tight', className)} {...props} />;
}

export function CardContent({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('p-4 pt-0', className)} {...props} />;
}

/** A labelled number. The unit of the summary strip above a run. */
export function Stat({
  label,
  value,
  hint,
}: {
  label: string;
  value: React.ReactNode;
  hint?: string;
}) {
  return (
    <div className="flex flex-col gap-0.5">
      <span className="text-xs uppercase tracking-wide text-muted-foreground">{label}</span>
      <span className="text-lg font-semibold tabular-nums">{value}</span>
      {hint ? <span className="text-xs text-muted-foreground">{hint}</span> : null}
    </div>
  );
}

/**
 * A copyable id.
 *
 * Trace and correlation ids are the whole point of this system, and the thing
 * people do with them is paste them somewhere else — into Grafana, into a
 * ticket. Showing a truncated id that cannot be copied in full would make that
 * the one annoying step.
 */
export function IdChip({
  label,
  value,
  full,
}: {
  label?: string;
  value: string | null | undefined;
  full?: string;
}) {
  const [copied, setCopied] = React.useState(false);
  if (!value) return <span className="text-muted-foreground">—</span>;

  const copyable = full ?? value;
  return (
    <button
      type="button"
      title={`${label ? `${label}: ` : ''}${copyable} (click to copy)`}
      onClick={() => {
        void navigator.clipboard?.writeText(copyable).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1200);
        });
      }}
      className="id rounded bg-muted px-1.5 py-0.5 text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
    >
      {copied ? 'copied' : value}
    </button>
  );
}

export function Spinner({ className }: { className?: string }) {
  return (
    <span
      aria-hidden
      className={cn(
        'inline-block h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent',
        className,
      )}
    />
  );
}

export function EmptyState({ title, hint }: { title: string; hint?: string }) {
  return (
    <div className="flex flex-col items-center gap-1 rounded-lg border border-dashed border-border p-10 text-center">
      <p className="text-sm font-medium">{title}</p>
      {hint ? <p className="max-w-md text-xs text-muted-foreground">{hint}</p> : null}
    </div>
  );
}
