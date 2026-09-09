<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * What a value supports, derived from the object a query actually built.
 *
 * The third surface, and the one the old list cost the most. `Whitelist`
 * offered seventeen comparisons plus `as`, `asc` and `desc`; the object behind
 * `ref('age')` offers over a hundred and thirty fluent methods — `plus`,
 * `minus`, `multiply`, `divide`, `round`, `concat`, `regexReplace`,
 * `dateFormat`, `sprintf`, `trim`, `isNumeric` — and `over`/`window` on an
 * aggregation, which is how a group's answer is put back beside the rows it
 * was taken over.
 *
 * So "this language has no arithmetic" was never true of Flow. It was true of
 * the list, and the list is gone: `ref('sib_sp')->plus(ref('parch'))->plus(lit(1))`
 * is a family size, in the query, because the engine could always do it.
 *
 * ## The rules
 *
 * Admitted when the method **returns a value** — one of the namespaces below,
 * or `static`/`self` for the fluent chains — and no parameter is one
 * [`Admission`] refuses. [`Registry::DECLINED`] applies here too, and has to:
 * `equals` and `notEquals` are methods as well as functions, and a rule that
 * refused the function while the method stayed reachable would be a rule that
 * had stopped meaning anything.
 *
 * The method is reflected on the object in hand, so what is reachable depends
 * on what the query built: an aggregation offers `over`, a window offers
 * `partitionBy`, a reference offers everything. Nothing is enumerated and
 * nothing is called by a name this class did not first look up on that exact
 * class.
 */
final class Values
{
    /**
     * Return types that mean "still a value".
     *
     * `Flow\ETL\Join\` is here for `identical()` and the comparisons a join
     * expression is built from: they compose values and open nothing.
     */
    private const array RETURNS = [
        'Flow\ETL\Function\\',
        'Flow\ETL\Row\\',
        'Flow\ETL\Window',
        'Flow\ETL\Join\\',
    ];

    /** @var array<class-string, array<string, \ReflectionMethod>> */
    private static array $byClass = [];

    /** @return array<string, \ReflectionMethod> */
    public static function methods(object $value): array
    {
        $class = $value::class;

        if (isset(self::$byClass[$class])) {
            return self::$byClass[$class];
        }

        $admitted = [];

        foreach ((new \ReflectionClass($value))->getMethods(\ReflectionMethod::IS_PUBLIC) as $method) {
            $name = $method->getName();

            if ($method->isStatic() || \str_starts_with($name, '__') || Registry::declined($name) !== null) {
                continue;
            }

            // A method that hands back the class it is declared on is a
            // fluent chain — `->as()`, `->over()`, `->partitionBy()` — and the
            // value is still the value it was. That is what `self` and
            // `static` say, and [`Admission::returned`] has already resolved
            // them, because PHP 8.5 reports the class and 8.3 reports the
            // word. Written as a comparison rather than as those two spellings
            // so that `Statistic::over(): static` and Flow's own chains are
            // one rule: this service's value classes are not in the namespaces
            // below and are values all the same.
            $returns = Admission::returned($method);

            if ($returns !== $method->getDeclaringClass()->getName() && !Admission::returns($returns, self::RETURNS)) {
                continue;
            }

            if (Admission::refuses($method) !== null) {
                continue;
            }

            $admitted[$name] = $method;
        }

        return self::$byClass[$class] = $admitted;
    }

    public static function method(object $value, string $name): ?\ReflectionMethod
    {
        return self::methods($value)[$name] ?? null;
    }

    /** @return list<string> */
    public static function names(object $value): array
    {
        $names = \array_keys(self::methods($value));
        \sort($names);

        return $names;
    }
}
