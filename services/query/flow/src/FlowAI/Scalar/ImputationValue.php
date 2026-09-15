<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI\Scalar;

use Aiwatcher\Flow\FlowAI\FittedState;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\ScalarFunctionChain;
use Flow\ETL\Row;
use Flow\Types\Type;

/** Only the missing engine primitive: lookup of a fitted group with fallback.
 * Null selection and indicators remain native coalesce/isNull/when expressions.
 */
final class ImputationValue implements ScalarFunction
{
    use ScalarFunctionChain;

    /** @param list<ScalarFunction> $groups */
    public function __construct(
        private readonly array $groups,
        private readonly FittedState $state,
        private readonly ScalarFunction $value,
        private readonly bool $median,
    ) {}

    public function children(): array
    {
        return [$this->value, ...$this->groups];
    }

    public function withChildren(array $children): static
    {
        return new self(\array_slice($children, 1), $this->state, $children[0], $this->median);
    }

    public function returns(): Type
    {
        return $this->median ? \Flow\Types\DSL\type_float() : $this->value->returns();
    }

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
