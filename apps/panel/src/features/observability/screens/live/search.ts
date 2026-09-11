import { attributeSearchSchema } from '@/features/observability/lib/selection-params';
import { windowSearchSchema } from '@/shared/components/time-range';
import { z } from 'zod';

/**
 * Watching a selection of agents work, as it happens: from the log while a run
 * is going, where Explore answers from the read model once it has finished.
 *
 * The server applies the selection, and model, tool and status are named as not
 * followed rather than sent — CLAUDE.md's Panel section says why. The feed is
 * bounded, and Pause freezes the rendering, never the subscription (ADR_0004,
 * amended).
 */

export const searchSchema = z.object({
  ...windowSearchSchema,
  ...attributeSearchSchema,
});
