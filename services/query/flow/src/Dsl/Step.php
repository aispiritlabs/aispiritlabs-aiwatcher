<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/** One `->method(...)` in the chain. */
final readonly class Step
{
    /** @param list<Argument> $args */
    public function __construct(
        public string $name,
        public array $args,
        public int $column,
    ) {}
}
