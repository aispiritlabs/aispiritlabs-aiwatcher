<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

use Flow\ETL\DataFrame;
use Flow\ETL\DataFrame\GroupedDataFrame;

/**
 * The steps a pipeline may take, derived from `DataFrame` rather than listed.
 *
 * [`Registry`]'s rule, applied to methods: a step is admitted when it **returns
 * a frame** and no parameter is one [`Admission`] refuses. On Flow 0.43 that is
 * 24 of the 53 public methods — `join`, `crossJoin`, `offset`, `until`,
 * `batchBy` among them — and refuses everything that could write a file, open a
 * source or run somebody's code: `write` and `load` take a `Loader`, `map` and
 * `forEach` a `callable`, `transform` a `Transformer`.
 *
 * The hand-written list this replaces had fourteen steps. Nothing chose those
 * fourteen except the order somebody needed them in, which is why `join()` was
 * missing for no reason that survived being written down.
 *
 * A name from a query selects a `ReflectionMethod` **in a map built here** and
 * is invoked against a `DataFrame`. The property ADR_0008 cares about is
 * unchanged: the reachable set is fixed, small and derived from types, and none
 * of it can reach a file, a socket or a callable. A `match` gave that set by
 * hand; this gives it by rule, and the rule stays right when Flow adds a method.
 */
final class Frame
{
    /**
     * What a frame method has to return to be a step.
     *
     * `count()`, `schema()` and `display()` are refused by this alone: a step
     * is something the next step continues from.
     */
    private const array RETURNS = [DataFrame::class, GroupedDataFrame::class];

    /**
     * Admitted by signature, refused anyway, with the reason.
     *
     * `cache(?string $id)` passes every type rule — a nullable string is not a
     * path as far as a signature is concerned — and writes files under a name
     * the query chose. This is the escape hatch for exactly that: a rule
     * cannot see what a string will be used for, and the answer is a named
     * refusal with the reason rather than a wider rule that catches innocent
     * strings.
     */
    public const array DECLINED = [
        'cache' =>
            'It writes cache files under an id taken from the query, and a query does not decide '
                . 'what this service puts on disk.',
    ];

    /**
     * Steps this service implements itself rather than passing to Flow.
     *
     * `read` is the catalog's, `write` means "give the rows back" and never
     * reaches `DataFrame::write`, and `run`/`fetch` are accepted so a query
     * written for the CLI pastes in unchanged. They are here so that "is this
     * a step" has one answer.
     */
    public const array OWN = [
        'read',
        'write',
        'run',
        'fetch',
        'trainTestSplit',
        'imputeMissing',
        'oneHotEncode',
        'labelEncode',
    ];

    /** @var array<string, \ReflectionMethod>|null */
    private static ?array $methods = null;

    /** @return array<string, \ReflectionMethod> */
    public static function methods(): array
    {
        if (self::$methods !== null) {
            return self::$methods;
        }

        $admitted = [];

        foreach ((new \ReflectionClass(DataFrame::class))->getMethods(\ReflectionMethod::IS_PUBLIC) as $method) {
            $name = $method->getName();

            if ($method->isStatic() || \str_starts_with($name, '__') || isset(self::DECLINED[$name])) {
                continue;
            }

            if (!Admission::returns(Admission::returned($method), self::RETURNS)) {
                continue;
            }

            if (Admission::refuses($method) !== null) {
                continue;
            }

            $admitted[$name] = $method;
        }

        return self::$methods = $admitted;
    }

    public static function has(string $name): bool
    {
        return isset(self::methods()[$name]);
    }

    public static function method(string $name): ?\ReflectionMethod
    {
        return self::methods()[$name] ?? null;
    }

    /** Whether a name is a step at all — Flow's or this service's own. */
    public static function isStep(string $name): bool
    {
        return self::has($name) || \in_array($name, self::OWN, true);
    }

    public static function declined(string $name): ?string
    {
        return self::DECLINED[$name] ?? null;
    }

    /** @return list<string> Every step a query may write, for a "did you mean". */
    public static function names(): array
    {
        $names = [...\array_keys(self::methods()), ...self::OWN];
        \sort($names);

        return $names;
    }
}
