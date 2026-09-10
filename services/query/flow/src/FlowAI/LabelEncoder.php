<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI;

final class LabelEncoder
{
    private ?array $classes = null;

    public function fit(array $labels): self
    {
        if (\in_array(null, $labels, true)) {
            throw new \InvalidArgumentException('Target labels cannot be missing.');
        }
        $this->classes = Category::vocabulary($labels);
        return $this;
    }

    public function transform(array $labels): array
    {
        $this->toState();
        return \array_map($this->encode(...), $labels);
    }

    public function encode(mixed $label): int
    {
        $classes = $this->classes ?? throw new \LogicException('Fit LabelEncoder before transform.');
        $index = \array_search(Category::key($label), $classes, true);
        if ($index === false) {
            throw new \InvalidArgumentException('Unknown or missing target label.');
        }
        return $index;
    }

    public function inverseTransform(array $labels): array
    {
        $classes = $this->classes ?? throw new \LogicException('Fit LabelEncoder before transform.');
        return \array_map(static function (mixed $label) use ($classes): mixed {
            if (!\is_int($label) || !\array_key_exists($label, $classes)) {
                throw new \InvalidArgumentException('Unknown encoded target label.');
            }
            return \json_decode($classes[$label], true, 512, \JSON_THROW_ON_ERROR);
        }, $labels);
    }

    public function toState(): array
    {
        return [
            'version' => 1,
            'kind' => 'label',
            'classes' => $this->classes ?? throw new \LogicException('Fit LabelEncoder before exporting state.'),
        ];
    }

    public static function fromState(array $state): self
    {
        if (($state['version'] ?? null) !== 1 || ($state['kind'] ?? null) !== 'label') {
            throw new \InvalidArgumentException('Unsupported LabelEncoder state.');
        }
        if (!\is_array($state['classes'] ?? null)) {
            throw new \InvalidArgumentException('Invalid LabelEncoder classes.');
        }
        Category::validate($state['classes']);
        $encoder = (new self())->fit(\array_map(static fn(string $value): mixed => \json_decode(
            $value,
            true,
            512,
            \JSON_THROW_ON_ERROR,
        ), $state['classes']));
        if ($encoder->toState() !== $state) {
            throw new \InvalidArgumentException('Noncanonical LabelEncoder state.');
        }
        return $encoder;
    }
}
