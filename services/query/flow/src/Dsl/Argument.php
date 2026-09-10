<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/** One argument, positional or named (`truncate: false`). */
final readonly class Argument
{
    public function __construct(
        public ?string $name,
        public Node $value,
    ) {}
}
