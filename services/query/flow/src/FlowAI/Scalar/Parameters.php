<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;
use Flow\Types\Type;

use function Flow\Types\DSL\type_array;

final class Parameters implements ScalarFunction
{
    use ScalarFunctionChain;

    public function __construct(
        private readonly FittedState $state,
    ) {}

    public function children(): array
    {
        return [];
    }

    public function withChildren(array $children): static
    {
        return new self($this->state);
    }

    public function returns(): Type
    {
        return type_array();
    }

    public function eval(Row $row, FlowContext $context): array
    {
        return $this->state->parameters();
    }
}
