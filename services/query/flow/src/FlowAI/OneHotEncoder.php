<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI;

/** Native PHP encoder; fit once on training rows, then reuse on validation/inference. */
final class OneHotEncoder
{
    private ?array $categories = null;

    public function __construct(
        private readonly array $columns,
        private readonly string $handleUnknown = 'ignore',
    ) {
        if ($columns === [] || !\array_is_list($columns)) {
            throw new \InvalidArgumentException('OneHotEncoder columns must be nonempty and unique.');
        }
        foreach ($columns as $column) {
            if (!\is_string($column) || $column === '') {
                throw new \InvalidArgumentException('OneHotEncoder columns must be names.');
            }
        }
        if (\count(\array_unique($columns)) !== \count($columns)) {
            throw new \InvalidArgumentException('OneHotEncoder columns must be unique.');
        }
        if (!\in_array($handleUnknown, ['ignore', 'error'], true)) {
            throw new \InvalidArgumentException('handleUnknown must be ignore or error.');
        }
    }

    public function fit(array $rows): self
    {
        return $this->fitColumns(\array_map(static fn(string $column): array => \array_map(
            static fn(array $row): mixed => Category::value($row, $column),
            $rows,
        ), $this->columns));
    }

    /** Fit column vectors, without materializing records from a DataFrame. */
    public function fitColumns(array $values): self
    {
        if (\count($values) !== \count($this->columns)) {
            throw new \InvalidArgumentException('OneHotEncoder needs one vector per column.');
        }
        $this->categories = \array_map(Category::vocabulary(...), $values);
        return $this;
    }

    public function transform(array $rows): array
    {
        $this->toState();
        return \array_map(fn(array $row): array => $this->encode(\array_map(
            static fn(string $column): mixed => Category::value($row, $column),
            $this->columns,
        )), $rows);
    }

    /** Apply a fitted vocabulary to one tuple of scalar expression results. */
    public function encode(array $values): array
    {
        $categories = $this->categories ?? throw new \LogicException('Fit OneHotEncoder before transform.');
        if (\count($values) !== \count($this->columns)) {
            throw new \InvalidArgumentException('OneHotEncoder needs one value per column.');
        }
        $features = [];
        foreach ($this->columns as $index => $column) {
            $key = Category::key($values[$index]);
            if ($this->handleUnknown === 'error' && !\in_array($key, $categories[$index], true)) {
                throw new \InvalidArgumentException("Unknown category for '{$column}': {$key}");
            }
            foreach ($categories[$index] as $category) {
                $features[$column . '=' . $category] = $category === $key ? 1.0 : 0.0;
            }
        }
        return $features;
    }

    public function toState(): array
    {
        return [
            'version' => 1,
            'kind' => 'one_hot',
            'columns' => $this->columns,
            'handle_unknown' => $this->handleUnknown,
            'categories' => $this->categories ?? throw new \LogicException('Fit OneHotEncoder before exporting state.'),
        ];
    }

    public static function fromState(array $state): self
    {
        if (($state['version'] ?? null) !== 1 || ($state['kind'] ?? null) !== 'one_hot') {
            throw new \InvalidArgumentException('Unsupported OneHotEncoder state.');
        }
        if (!\is_array($state['columns'] ?? null) || !\is_string($state['handle_unknown'] ?? null)) {
            throw new \InvalidArgumentException('Invalid OneHotEncoder configuration.');
        }
        $encoder = new self($state['columns'], $state['handle_unknown']);
        $categories = $state['categories'] ?? null;
        if (
            !\is_array($categories)
            || !\array_is_list($categories)
            || \count($categories) !== \count($state['columns'])
        ) {
            throw new \InvalidArgumentException('Invalid OneHotEncoder categories.');
        }
        foreach ($categories as $values) {
            if (!\is_array($values)) {
                throw new \InvalidArgumentException('Invalid OneHotEncoder vocabulary.');
            }
            Category::validate($values);
        }
        $encoder->categories = $categories;
        return $encoder;
    }
}
