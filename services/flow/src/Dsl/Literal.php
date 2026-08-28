<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/** A string, number, boolean or null written directly in the query. */
final readonly class Literal implements Node
{
    public function __construct(
        public string|int|float|bool|null $value,
        private int $column,
    ) {}

    public function column(): int
    {
        return $this->column;
    }
}
