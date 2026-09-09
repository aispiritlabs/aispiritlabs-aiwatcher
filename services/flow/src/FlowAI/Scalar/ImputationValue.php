<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;

/** Only the missing engine primitive: lookup of a fitted group with fallback.
 * Null selection and indicators remain native coalesce/isNull/when expressions.
 */
final class ImputationValue extends ScalarFunctionChain
{
    /** @param list<ScalarFunction> $groups */
    public function __construct(
        private readonly array $groups,
        private readonly FittedState $state,
    ) {}

    public function eval(Row $row, FlowContext $context): mixed
    {
        $key = \json_encode(
            \array_map(static fn(ScalarFunction $group): mixed => $group->eval($row, $context), $this->groups),
            \JSON_THROW_ON_ERROR,
        );
        $parameters = $this->state->parameters();
        return $parameters['groups'][$key] ?? $parameters['fallback'];
    }
}
