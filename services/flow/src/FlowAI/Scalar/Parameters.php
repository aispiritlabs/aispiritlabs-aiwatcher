<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;

final class Parameters extends ScalarFunctionChain
{
    public function __construct(
        private readonly FittedState $state,
    ) {}

    public function eval(Row $row, FlowContext $context): array
    {
        return $this->state->parameters();
    }
}
