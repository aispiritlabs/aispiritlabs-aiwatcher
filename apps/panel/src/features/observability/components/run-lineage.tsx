import * as React from 'react';

import { factsOf, type Span } from '@/features/observability/lib/span-facts';
import { ModelVersionReference } from '@/shared/components/lineage-reference';
import { PromptRefLink } from '@/shared/components/prompt-bits';

/**
 * What a run ran on, as links out of the log.
 *
 * A run's calls name a registered prompt version and, where a registry served
 * them, a model version — both of which outlive the log's retention and are
 * where a reader goes next from an answer that was wrong. Until this, both
 * were one click deep in a single span's attribute table, so a run of twenty
 * calls made you open them to find out whether they all used the same prompt.
 *
 * This is a **read of what is already on the page**, not a fold: the run's
 * spans arrived whole with `GET /api/v1/runs/{id}`, and distinct references
 * across them are the same kind of derivation as the attribute table's. It
 * counts nothing and totals nothing — where a number over a population is
 * wanted, that is the server's, and there is no route that groups runs by
 * prompt at all.
 */
export function RunLineage({ spans }: { spans: Span[] }) {
  const { prompts, models } = React.useMemo(() => distinct(spans), [spans]);
  if (prompts.length === 0 && models.length === 0) return null;

  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
      {prompts.length > 0 ? (
        <span className="flex flex-wrap items-center gap-2">
          <span>prompt{prompts.length > 1 ? 's' : ''}</span>
          {prompts.map((prompt) => (
            <PromptRefLink
              key={prompt.versionId}
              name={prompt.name}
              versionId={prompt.versionId}
            />
          ))}
        </span>
      ) : null}
      {models.length > 0 ? (
        <span className="flex flex-wrap items-center gap-2">
          <span>· model version{models.length > 1 ? 's' : ''}</span>
          {models.map((model) => (
            <ModelVersionReference
              key={`${model.name ?? ''}@${model.version}`}
              model={model.name}
              version={model.version}
            />
          ))}
        </span>
      ) : null}
    </div>
  );
}

/**
 * Each reference once, in the order the run first used it.
 *
 * First-seen rather than sorted, because the order calls were made in is the
 * order somebody read the waterfall in, and re-sorting would put the prompt of
 * the last call first on a run whose first call is the one that went wrong.
 */
function distinct(spans: Span[]) {
  const prompts = new Map<string, { name?: string; versionId: string }>();
  const models = new Map<string, { name?: string; version: string }>();
  for (const span of spans) {
    const facts = factsOf(span);
    if (facts.prompt?.versionId && !prompts.has(facts.prompt.versionId)) {
      prompts.set(facts.prompt.versionId, {
        name: facts.prompt.name,
        versionId: facts.prompt.versionId,
      });
    }
    if (facts.model) {
      const key = `${facts.model.name ?? ''}@${facts.model.version}`;
      if (!models.has(key)) models.set(key, facts.model);
    }
  }
  return { prompts: [...prompts.values()], models: [...models.values()] };
}
