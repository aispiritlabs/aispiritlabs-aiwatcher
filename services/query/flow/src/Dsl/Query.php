<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/** A parsed query: the chain of steps, and nothing evaluated. */
final readonly class Query
{
    /** @param list<Step> $steps */
    public function __construct(
        public array $steps,
    ) {}

    public function stepNamed(string $name): ?Step
    {
        foreach ($this->steps as $step) {
            if ($step->name === $name) {
                return $step;
            }
        }

        return null;
    }
}
