<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI;

/** Run-local fitted parameters shared by the fit step and scalar expressions.
 * Contains no Rows, closures, resources or service clients; serializable with the plan.
 */
final class FittedState
{
    private ?array $parameters = null;
    private ?OneHotEncoder $oneHot = null;
    private ?LabelEncoder $label = null;

    public function set(array $parameters): void
    {
        $this->oneHot = ($parameters['kind'] ?? null) === 'one_hot' ? OneHotEncoder::fromState($parameters) : null;
        $this->label = ($parameters['kind'] ?? null) === 'label' ? LabelEncoder::fromState($parameters) : null;
        $this->parameters = $parameters;
    }

    public function parameters(): array
    {
        return $this->parameters ?? throw new \LogicException('Fit preparation before applying it.');
    }

    public function oneHot(): OneHotEncoder
    {
        return $this->oneHot ?? throw new \LogicException('Fit OneHotEncoder before applying it.');
    }

    public function label(): LabelEncoder
    {
        return $this->label ?? throw new \LogicException('Fit LabelEncoder before applying it.');
    }
}
