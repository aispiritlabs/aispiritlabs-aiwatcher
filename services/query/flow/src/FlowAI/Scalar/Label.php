<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;
use Flow\Types\Type;

use function Flow\Types\DSL\type_integer;

final class Label implements ScalarFunction
{
    use ScalarFunctionChain;

    public function __construct(
        private readonly ScalarFunction $value,
        private readonly FittedState $state,
    ) {}

    public function children(): array
    {
        return [$this->value];
    }

    public function withChildren(array $children): static
    {
        return new self($children[0], $this->state);
    }

    public function returns(): Type
    {
        return type_integer();
    }

    public function eval(Row $row, FlowContext $context): int
    {
        return $this->state->label()->encode($this->value->eval($row, $context));
    }
}
