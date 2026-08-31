<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * What a query is allowed to say.
 *
 * This is the security boundary, so it is a list of what is permitted, never a
 * list of what is forbidden. Anything absent is a parse error; there is no path
 * from query text to a call that is not named here.
 *
 * Names are grouped by what they may appear as, because position matters:
 * `count` is meaningful inside `aggregate()` and meaningless as a pipeline
 * step, and accepting it in both places would widen the surface for nothing.
 */
final class Whitelist
{
    /** Pipeline steps, in the order they may appear after `data_frame()`. */
    public const array METHODS = [
        'read',
        'select',
        'drop',
        'dropDuplicates',
        'rename',
        'filter',
        'withEntry',
        'groupBy',
        'aggregate',
        'sortBy',
        'limit',
        'write',
        'run',
        'fetch',
    ];

    /** How a column is named. */
    public const array REFERENCES = ['ref', 'col', 'lit'];

    /** Only valid inside `aggregate()`. */
    public const array AGGREGATIONS = [
        'count',
        'sum',
        'average',
        'min',
        'max',
        'first',
        'last',
        'collect',
        'collect_unique',
    ];

    /** Row-level functions, valid wherever a value is. */
    public const array SCALARS = [
        'array_get',
        'array_expand',
        'concat',
        'concat_ws',
        'lower',
        'upper',
        'cast',
        'coalesce',
        'size',
        'round',
        'when',
        'exists',
        'not',
        'between',
        'regex_match',
        'split',
        'hash',
        'identical',
        'optional',
        'all',
        'any',
    ];

    /**
     * Sinks. All of them mean the same thing here: return the rows.
     *
     * A query written for the CLI ends in `write(to_output(...))`, and that
     * should keep working when pasted into the panel rather than being an error
     * about an unsupported sink. `to_output(truncate: false)` carries one real
     * instruction — do not shorten the cells — which the panel honours.
     */
    public const array SINKS = ['to_output', 'to_array', 'to_memory'];

    /**
     * Methods a reference supports.
     *
     * Flow puts these on the reference rather than on the surrounding call:
     * `ref('x')->desc()`, and `count(ref('run_id')->as('runs'))`. Modelling
     * them the same way is what lets the parser catch the alias-on-aggregation
     * mistake instead of passing it to Flow to die on.
     */
    public const array REFERENCE_METHODS = [
        'as',
        'asc',
        'desc',
        ...self::COMPARISONS,
    ];

    /**
     * Comparisons, which Flow also puts on the reference.
     *
     * The standalone `equal()` compares two *columns*; comparing a column to a
     * value is `ref('status')->equals(lit('failed'))`. That trips everyone up
     * once, so both forms are available and the shapes are checked here rather
     * than surfacing as a `TypeError` from inside Flow.
     */
    public const array COMPARISONS = [
        'same',
        'notSame',
        'greaterThan',
        'greaterThanEqual',
        'lessThan',
        'lessThanEqual',
        'isIn',
        'contains',
        'startsWith',
        'endsWith',
        'isNull',
        'isNotNull',
        'isTrue',
        'isFalse',
        'isEmpty',
    ];

    /** Whether a reference method takes exactly one argument. */
    public static function comparisonTakesArgument(string $name): bool
    {
        return !\in_array($name, ['isNull', 'isNotNull', 'isTrue', 'isFalse', 'isEmpty'], true);
    }

    /**
     * Names deliberately left out, and why.
     *
     * A whitelist is also a place to decline something that exists and works
     * badly here. Flow 0.43's loose comparisons fall through to an array
     * comparison when either side is null, which makes them silently wrong on
     * nullable data — measured on three rows where one column is null:
     *
     * ```text
     * ref('op')->equals(lit('execute_tool'))     -> ['execute_tool', null]   wrong
     * ref('op')->notEquals(lit('execute_tool'))  -> ['chat']                 wrong
     * ref('op')->same(lit('execute_tool'))       -> ['execute_tool']         right
     * ref('op')->notSame(lit('execute_tool'))    -> ['chat', null]           right
     * ```
     *
     * Every column in every dataset here is nullable, so offering these by name
     * would be offering a filter that quietly returns the wrong rows. They are
     * refused with the reason rather than silently missing, because "unknown
     * function" would send someone looking for a typo.
     */
    public const array DECLINED = [
        'equals' =>
            'Use ->same(...). Flow\'s ->equals() compares loosely and, when either side is '
                . 'null, falls through to an array comparison that matches anything — and every column '
                . 'in these datasets can be null.',
        'notEquals' =>
            'Use ->notSame(...). Flow\'s ->notEquals() drops rows where the column is '
                . 'null, rather than keeping them as "not equal".',
        'equal' =>
            'Use ref(\'a\')->same(ref(\'b\')) or ->same(lit(\'value\')). The standalone '
                . 'equal() compares loosely and mishandles nulls.',
    ];

    public static function declined(string $name): ?string
    {
        return self::DECLINED[$name] ?? null;
    }

    public static function isMethod(string $name): bool
    {
        return \in_array($name, self::METHODS, true);
    }

    public static function isValueFunction(string $name): bool
    {
        return (
            \in_array($name, self::REFERENCES, true)
            || \in_array($name, self::SCALARS, true)
            || \in_array($name, self::AGGREGATIONS, true)
            || \in_array($name, self::SINKS, true)
        );
    }

    public static function isAggregation(string $name): bool
    {
        return \in_array($name, self::AGGREGATIONS, true);
    }

    public static function isSink(string $name): bool
    {
        return \in_array($name, self::SINKS, true);
    }

    /**
     * Everything a query may name, for an error message worth reading.
     *
     * @return list<string>
     */
    public static function everything(): array
    {
        return \array_merge(
            self::METHODS,
            self::REFERENCES,
            self::AGGREGATIONS,
            self::SCALARS,
            self::SINKS,
            self::REFERENCE_METHODS,
        );
    }

    /** The nearest allowed name, when there is one close enough to suggest. */
    public static function nearest(string $name): ?string
    {
        $closest = null;
        $distance = \PHP_INT_MAX;

        foreach (self::everything() as $known) {
            $candidate = \levenshtein($name, $known);

            if ($candidate < $distance) {
                $distance = $candidate;
                $closest = $known;
            }
        }

        return $distance <= 3 ? $closest : null;
    }
}
