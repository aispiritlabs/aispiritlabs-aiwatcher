<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * A bare identifier used as a value.
 *
 * The only place this is legal is naming a dataset — `read(default)`. Anywhere
 * else the builder rejects it, so a stray identifier cannot become a silent
 * null.
 */
final readonly class Bareword implements Node
{
    public function __construct(
        public string $name,
        private int $column,
    ) {}

    public function column(): int
    {
        return $this->column;
    }
}
