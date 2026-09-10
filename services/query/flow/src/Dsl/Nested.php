<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * A whole query written where a value goes, which is how a join names its
 * right-hand side.
 *
 * ```text
 * ->join(
 *     data_frame()->read(hub_rows, dataset: 'phihung/titanic')->groupBy(ref('status'))->aggregate(median(ref('age'))),
 *     on: identical(ref('status'), ref('status')),
 *     type: 'left'
 * )
 * ```
 *
 * The right side is a query rather than a dataset name because it usually is
 * one: joining a *table* is rare, and joining what another query worked out —
 * a median per group, a count per agent — is the whole reason to have this.
 *
 * It reads through the same catalog, the same window and the same admission
 * rules as the query around it, so nothing here widens what a query may reach.
 * What it does widen is memory: the right side is materialised, which is why
 * `PipelineBuilder` caps it the way it caps everything else.
 */
final readonly class Nested implements Node
{
    public function __construct(
        public Query $query,
        private int $column,
    ) {}

    public function column(): int
    {
        return $this->column;
    }
}
