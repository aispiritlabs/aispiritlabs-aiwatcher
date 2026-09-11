import { useQuery } from '@tanstack/react-query';

import { getOptimization } from '@/api/generated/sdk.gen';
import type { OptimizationSummary, PromptVersion } from '@/api/generated/types.gen';
import { answerOrNone } from '@/shared/lib/result';

/** What a refusal carries: the recorded reason, or `null` when no record exists. */
export type RecordedVerdict = Pick<OptimizationSummary, 'reason' | 'variables_lost'>;

/**
 * Whether `production` may point at a version, as the registry recorded it.
 *
 * `open` — a version somebody wrote, or a candidate the verdict admitted.
 * `reading` — a candidate whose optimisation fell out of the head's capped
 * index and is being fetched. `refused` — a candidate the verdict rejected, or
 * one with no record at all, which `check_admitted` refuses as well: an
 * unwritten verdict is not an admission. `unread` — the record could not be
 * fetched, so the button stays and the registry decides.
 */
export type Promotion =
  | { state: 'open' }
  | { state: 'reading' }
  | { state: 'refused'; optimizationId: string; verdict: RecordedVerdict | null }
  | { state: 'unread'; failure: unknown };

/**
 * Read the verdict `PUT /labels/production` will answer to.
 *
 * Reads the recorded `outcome` and nothing else — the verdict is computed by
 * the server, and a second computation here would drift from it. The verdict
 * that counts is the one the version's *origin* names, not whichever
 * optimisation last produced the same text, because that is the one
 * `check_admitted` looks up.
 */
export function usePromotion(
  name: string,
  version: PromptVersion | undefined,
  indexed: OptimizationSummary[],
): Promotion {
  const optimizationId = version?.origin === 'optimized' ? version.optimization_id : undefined;
  const summary = optimizationId
    ? indexed.find((record) => record.optimization_id === optimizationId)
    : undefined;

  const record = useQuery({
    queryKey: ['optimization-verdict', name, optimizationId],
    enabled: Boolean(optimizationId) && !summary,
    queryFn: async () =>
      answerOrNone(
        await getOptimization({ path: { name, optimization_id: optimizationId as string } }),
        'failed to read the optimisation',
      ),
  });

  if (!optimizationId) return { state: 'open' };
  if (summary) return verdictOf(optimizationId, summary);
  if (record.isError) return { state: 'unread', failure: record.error };
  if (record.data === undefined) return { state: 'reading' };
  return record.data === null
    ? { state: 'refused', optimizationId, verdict: null }
    : verdictOf(optimizationId, record.data);
}

function verdictOf(
  optimizationId: string,
  record: RecordedVerdict & Pick<OptimizationSummary, 'outcome'>,
): Promotion {
  return record.outcome === 'admitted'
    ? { state: 'open' }
    : { state: 'refused', optimizationId, verdict: record };
}
