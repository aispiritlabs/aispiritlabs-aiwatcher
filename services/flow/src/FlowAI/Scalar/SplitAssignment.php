<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\Category;
use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;

final class SplitAssignment extends ScalarFunctionChain
{
    public function __construct(
        private readonly ScalarFunction $id,
        private readonly FittedState $state,
    ) {}

    public function eval(Row $row, FlowContext $context): string
    {
        $parameters = $this->state->parameters();
        return isset($parameters['validation'][Category::key($this->id->eval($row, $context))])
            ? 'validation'
            : 'train';
    }
}
