/**
 * Comparability, as a control rather than as a fourth sentence.
 *
 * Both halves of this area answer it and the two rules are different, because
 * the evidence is: a folded report compares five optional strings a producer
 * may not have sent, published evidence compares one content address that pins
 * the cohort, the split, the suite, the scorer and the metric definitions
 * together. What a reader *does* with the answer is the same either way, so the
 * three words are one vocabulary — `aiwatcher_core::Comparability` — and this
 * is the one control that draws them.
 *
 * The panel implements none of the rule. It renders which of the three the
 * server chose, and says plainly that a delta is withheld rather than leaving
 * it absent, because an absent number reads as a number nobody measured rather
 * than as one nobody may claim.
 */
import type { Comparability } from '@/api/generated/types.gen';
import { cn } from '@/shared/lib/utils';

const SAYS: Record<Comparability, string> = {
  comparable: 'Deltas below are a like-for-like comparison.',
  unverified: 'Deltas are withheld: the evidence for a like-for-like comparison is missing.',
  incompatible:
    'Deltas are withheld: these two were not measured on the same thing, so a difference between them is not a change.',
};

export function ComparabilityControl({ value }: { value: Comparability }) {
  return (
    <div role="group" aria-label="Comparability">
      <div className="flex flex-wrap items-center gap-1">
        {(['comparable', 'unverified', 'incompatible'] as const).map((state) => (
          <span
            key={state}
            aria-current={state === value ? 'true' : undefined}
            className={cn(
              'rounded-md border px-2 py-0.5',
              state === value
                ? state === 'comparable'
                  ? 'border-primary bg-primary/10 font-medium text-foreground'
                  : 'border-warning bg-warning/10 font-medium text-foreground'
                : 'border-border/60 text-muted-foreground/60',
            )}
          >
            {state}
          </span>
        ))}
      </div>
      <p className="mt-1">{SAYS[value]}</p>
    </div>
  );
}
