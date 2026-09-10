<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;

final class OneHot extends ScalarFunctionChain
{
    /** @param list<ScalarFunction> $values */
    public function __construct(
        private readonly array $values,
        private readonly FittedState $state,
    ) {}

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
