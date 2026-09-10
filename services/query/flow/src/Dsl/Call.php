<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/** A whitelisted function call, e.g. `ref('agent')` or `count(...)`. */
final readonly class Call implements Node
{
    /** @param list<Argument> $args */
    public function __construct(
        public string $name,
        public array $args,
        private int $column,
        /** Set by a trailing `->as('name')`. */
        public ?string $alias = null,
        /** Set by a trailing `->asc()` or `->desc()`. */
        public ?string $order = null,
        /**
         * Comparisons chained onto the value, in order.
         *
         * `ref('status')->equals(lit('failed'))` is one of these. Kept as a
         * list rather than folded into the call so the builder applies them
         * through an explicit match, like everything else a query can name.
         *
         * @var list<Step>
         */
        public array $chain = [],
    ) {}

    public function column(): int
    {
        return $this->column;
    }
}
