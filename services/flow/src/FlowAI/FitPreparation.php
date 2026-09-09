<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI;

use Aiwatcher\Flow\Statistics\Descriptive;
use Flow\ETL\FlowContext;
use Flow\ETL\Row;
use Flow\ETL\Rows;
use Flow\ETL\Transformer;

/** Explicit fitting boundary: read selected Entry values and return the same Rows.
 * Stores only vocabularies, statistics or split IDs. Applying them belongs to
 * Scalar functions. Exact medians and stratification require a bounded collection;
 * they cannot be fitted independently per page without changing the result.
 */
final readonly class FitPreparation implements Transformer
{
    public function __construct(
        private string $operation,
        private array $columns,
        private array $options,
        private FittedState $state,
    ) {}

    public function transform(Rows $rows, FlowContext $context): Rows
    {
        if ($rows->count() === 0) {
            return $rows;
        }
        $parameters = match ($this->operation) {
            'trainTestSplit' => $this->split($rows),
            'imputeMissing' => $this->impute($rows),
            'oneHotEncode', 'labelEncode' => $this->vocabulary($rows),
        };
        $this->state->set($parameters);
        return $rows;
    }

    private function training(Row $row): bool
    {
        return (
            !isset($this->options['fitOn'])
            || $row->valueOf($this->options['fitOn']) === ($this->options['fitValue'] ?? 'train')
        );
    }

    private function vocabulary(Rows $rows): array
    {
        $vocabularies = \array_fill(0, \count($this->columns), []);
        foreach ($rows as $row) {
            if (!$this->training($row)) {
                continue;
            }
            foreach ($this->columns as $index => $column) {
                $vocabularies[$index][Category::key($row->valueOf($column))] = true;
            }
        }
        $categories = \array_map(static function (array $keys): array {
            $values = \array_map('strval', \array_keys($keys));
            \sort($values, \SORT_STRING);
            return $values;
        }, $vocabularies);
        return (
            $this->operation === 'oneHotEncode'
                ? [
                    'version' => 1,
                    'kind' => 'one_hot',
                    'columns' => $this->columns,
                    'handle_unknown' => $this->options['handleUnknown'] ?? 'ignore',
                    'categories' => $categories,
                ]
                : ['version' => 1, 'kind' => 'label', 'classes' => $categories[0]]
        );
    }

    private function split(Rows $rows): array
    {
        $groups = [];
        $ids = [];
        foreach ($rows as $row) {
            $id = Category::key($row->valueOf($this->columns[0]));
            if ($id === 'null' || isset($ids[$id])) {
                throw new \InvalidArgumentException('Split identifiers must be present and unique.');
            }
            $ids[$id] = true;
            $label = Category::key($row->valueOf($this->options['target']));
            $groups[$label][$id] = \hash('sha256', ($this->options['seed'] ?? 42) . ':' . $id);
        }
        $validation = [];
        $fraction = $this->options['fraction'] ?? 0.2;
        if (!\is_numeric($fraction) || $fraction <= 0 || $fraction >= 1) {
            throw new \InvalidArgumentException('Split fraction must be between 0 and 1.');
        }
        foreach ($groups as $group) {
            if (\count($group) < 2) {
                throw new \InvalidArgumentException('Stratified split needs at least two rows per target class.');
            }
            \asort($group, \SORT_STRING);
            $count = \min(\count($group) - 1, (int) \ceil(\count($group) * $fraction));
            foreach (\array_slice(\array_keys($group), 0, $count) as $id) {
                $validation[$id] = true;
            }
        }
        return ['validation' => $validation];
    }

    private function impute(Rows $rows): array
    {
        $groupColumns = isset($this->options['groupBy'])
            ? \array_map('trim', \explode(',', $this->options['groupBy']))
            : [];
        $groups = [];
        $all = [];
        foreach ($rows as $row) {
            if (!$this->training($row)) {
                continue;
            }
            $value = $row->valueOf($this->columns[0]);
            if ($value === null) {
                continue;
            }
            $key = \json_encode(\array_map($row->valueOf(...), $groupColumns), \JSON_THROW_ON_ERROR);
            $groups[$key][] = $value;
            $all[] = $value;
        }
        return ['fallback' => $this->reduce($all), 'groups' => \array_map($this->reduce(...), $groups)];
    }

    private function reduce(array $values): mixed
    {
        if ($values === []) {
            throw new \InvalidArgumentException('Imputation needs nonmissing training values.');
        }
        if (($this->options['strategy'] ?? 'median') === 'most_frequent') {
            $counts = \array_count_values(\array_map(Category::key(...), $values));
            \ksort($counts, \SORT_STRING);
            \arsort($counts, \SORT_NUMERIC);
            return \json_decode((string) \array_key_first($counts), true, 512, \JSON_THROW_ON_ERROR);
        }
        foreach ($values as $value) {
            if (!\is_int($value) && !\is_float($value)) {
                throw new \InvalidArgumentException('Median imputation requires numeric values.');
            }
        }
        return Descriptive::Median->of($values);
    }
}
