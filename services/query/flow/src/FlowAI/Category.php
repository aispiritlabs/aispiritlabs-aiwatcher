<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\FlowAI;

/** JSON scalar keys keep null, "null", 1 and "1" distinct. */
final class Category
{
    public static function key(mixed $value): string
    {
        if (!\is_scalar($value) && $value !== null) {
            throw new \InvalidArgumentException('FlowAI categories must be JSON scalars.');
        }
        return \json_encode(
            $value,
            \JSON_THROW_ON_ERROR | \JSON_UNESCAPED_UNICODE | \JSON_UNESCAPED_SLASHES | \JSON_PRESERVE_ZERO_FRACTION,
        );
    }

    public static function vocabulary(array $values): array
    {
        $keys = \array_values(\array_unique(\array_map(self::key(...), $values)));
        \sort($keys, \SORT_STRING);
        if ($keys === []) {
            throw new \InvalidArgumentException('FlowAI fit needs training values.');
        }
        return $keys;
    }

    public static function validate(array $keys): void
    {
        if ($keys === [] || !\array_is_list($keys)) {
            throw new \InvalidArgumentException('Invalid FlowAI vocabulary.');
        }
        foreach ($keys as $key) {
            if (!\is_string($key)) {
                throw new \InvalidArgumentException('Vocabulary entries must be JSON strings.');
            }
            self::key(\json_decode($key, true, 512, \JSON_THROW_ON_ERROR));
        }
        if (\count(\array_unique($keys)) !== \count($keys)) {
            throw new \InvalidArgumentException('Duplicate vocabulary entries.');
        }
    }

    public static function value(array $row, string $column): mixed
    {
        if (!\array_key_exists($column, $row)) {
            throw new \InvalidArgumentException("FlowAI input has no column '{$column}'.");
        }
        return $row[$column];
    }
}
