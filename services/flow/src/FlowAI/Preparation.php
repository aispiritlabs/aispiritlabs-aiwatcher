<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI;

use Flow\ETL\DataFrame;

use function Flow\ETL\DSL\coalesce;
use function Flow\ETL\DSL\lit;
use function Flow\ETL\DSL\ref;
use function Flow\ETL\DSL\when;

/** Compile preparation into native withEntry expressions, with a separate fit boundary. */
final readonly class Preparation
{
    public function __construct(
        private string $operation,
        private array $columns,
        private array $options,
    ) {}

    public function apply(DataFrame $frame): DataFrame
    {
        $state = new FittedState();
        if (isset($this->options['state'])) {
            $parameters = \json_decode($this->options['state'], true, 512, \JSON_THROW_ON_ERROR);
            $expectedKind = $this->operation === 'oneHotEncode' ? 'one_hot' : 'label';
            if (!\is_array($parameters) || ($parameters['kind'] ?? null) !== $expectedKind) {
                throw new \InvalidArgumentException('Saved encoder kind does not match the step.');
            }
            $state->set($parameters);
            if ($this->operation === 'oneHotEncode' && $state->parameters()['columns'] !== $this->columns) {
                throw new \InvalidArgumentException('Saved encoder columns do not match the step.');
            }
        } else {
            // Exact stratification and fitting vocabulary/group statistics need all
            // training pages. The transformer only fits; it returns the same Rows.
            // Inference with supplied state remains a streaming scalar projection.
            $frame = $frame
                ->collect()
                ->transform(new FitPreparation($this->operation, $this->columns, $this->options, $state));
        }
        $input = ref($this->columns[0]);
        if (isset($this->options['missingIndicator'])) {
            $frame = $frame->withEntry($this->options['missingIndicator'], when($input->isNull(), lit(1), lit(0)));
        }
        $value = match ($this->operation) {
            'trainTestSplit' => new Scalar\SplitAssignment($input, $state),
            'oneHotEncode' => new Scalar\OneHot(\array_map(ref(...), $this->columns), $state),
            'labelEncode' => when($input->isNull(), lit(null), new Scalar\Label($input, $state)),
            'imputeMissing' => coalesce(
                $input,
                new Scalar\ImputationValue(
                    \array_map(
                        ref(...),
                        isset($this->options['groupBy'])
                            ? \array_map('trim', \explode(',', $this->options['groupBy']))
                            : [],
                    ),
                    $state,
                ),
            ),
        };
        $frame = $frame->withEntry($this->options['output'], $value);
        if (isset($this->options['stateOutput'])) {
            $frame = $frame->withEntry($this->options['stateOutput'], new Scalar\Parameters($state));
        }
        return $frame;
    }
}
