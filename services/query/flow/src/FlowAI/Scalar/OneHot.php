<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;
use Flow\Types\Type;

use function Flow\Types\DSL\type_float;
use function Flow\Types\DSL\type_map;
use function Flow\Types\DSL\type_string;

final class OneHot implements ScalarFunction
{
    use ScalarFunctionChain;

    /** @param list<ScalarFunction> $values */
    public function __construct(
        private readonly array $values,
        private readonly FittedState $state,
    ) {}

    public function children(): array
    {
        return $this->values;
    }

    public function withChildren(array $children): static
    {
        return new self($children, $this->state);
    }

    public function returns(): Type
    {
        return type_map(type_string(), type_float());
    }

    public function eval(Row $row, FlowContext $context): array
    {
        return $this->state
            ->oneHot()
            ->encode(\array_map(static fn(ScalarFunction $value): mixed => $value->eval(
                $row,
                $context,
            ), $this->values));
    }
}
