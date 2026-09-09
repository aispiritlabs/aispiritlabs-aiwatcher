<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;

final class Label extends ScalarFunctionChain
{
    public function __construct(
        private readonly ScalarFunction $value,
        private readonly FittedState $state,
    ) {}

    public function eval(Row $row, FlowContext $context): int
    {
        return $this->state->label()->encode($this->value->eval($row, $context));
    }
}
