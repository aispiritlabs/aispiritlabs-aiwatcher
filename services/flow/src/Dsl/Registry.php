<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * What a query may name. The whole decision, and the only one.
 *
 * ADR 0008's rule is about **dispatch**: a name from a query never becomes a
 * callable. A string that selects a key in a map built here is exactly as safe
 * as one that selects a `match` arm, which is what lets this admit Flow's own
 * function namespace without widening the boundary by one call.
 *
 * Two vocabularies are not Flow's and are enums rather than lists, so neither
 * grows by accident: [`Sink`] and [`Statistics\Descriptive`].
 *
 * Three rules decide admission, and none is a name:
 *
 * - **The return type's namespace.** A query composes values; it does not open
 *   sources or write sinks, so `Flow\ETL\Loader`, `Extractor` and `Filesystem`
 *   are absent. The catalog owns what may be read, `write()` owns the sinks.
 * - **Any parameter accepting a callable** is refused — `call()` and
 *   `to_callable()` are the remote code execution this service exists to
 *   refuse, and refusing by signature covers whatever Flow adds next.
 * - **[`self::DECLINED`]**, which is about correctness rather than safety and
 *   keeps its own reasons.
 *
 * The return type is read as a **string**: `class_exists()` autoloads, and
 * `Flow\ETL\Function\Uuid` throws at load without `ramsey/uuid`.
 */
final class Registry
{
    /**
     * Return-type namespaces a query may name.
     *
     * Everything else is absent rather than declined: `Schema*` and the PHP
     * scalar helpers are harmless and answer no question a query asks, and
     * `Flow`, `FlowContext` and `Cache` are the pipeline's own machinery.
     */
    private const array ADMITTED = [
        'Flow\ETL\Function\\',
        'Flow\ETL\Row\\',
        'Flow\ETL\Window',
        // `identical()` and `join_on()`: a join's condition is a comparison
        // between two columns, which composes values and opens nothing. The
        // frame it joins *to* comes from the catalog, through a nested query,
        // and never from a name in this namespace.
        'Flow\ETL\Join\\',
    ];

    /**
     * Functions whose answer depends on when it ran.
     *
     * Admitted, and *reported*. A managed step that used one is not a pure
     * function of its inputs, so the answer carries `deterministic: false` and
     * the reactor declines to remember it — `ActivityResult::cacheable`, the
     * same seam a drifting window already reaches the cache through. Nothing
     * here stops the panel using them; ad-hoc queries are not cached at all.
     */
    public const array NONDETERMINISTIC = [
        'now',
        'random_string',
        'ulid',
        'uuid_v4',
        'uuid_v7',
    ];

    /**
     * Names Flow offers that a query may not use, and why.
     *
     * The one refusal here that is about correctness rather than safety. Flow
     * 0.43's loose comparisons fall through to an array comparison when either
     * side is null, which makes them silently wrong on nullable data —
     * measured on three rows where one column is null:
     *
     * ```text
     * ref('op')->equals(lit('execute_tool'))     -> ['execute_tool', null]   wrong
     * ref('op')->notEquals(lit('execute_tool'))  -> ['chat']                 wrong
     * ref('op')->same(lit('execute_tool'))       -> ['execute_tool']         right
     * ref('op')->notSame(lit('execute_tool'))    -> ['chat', null]           right
     * ```
     *
     * Every column in every dataset here is nullable, so admitting these would
     * be offering a filter that quietly returns the wrong rows. They are
     * refused *with the reason* rather than silently absent, because "unknown
     * function" would send somebody looking for a typo.
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

    /** @var array<string, \ReflectionFunction>|null Built once per request; see the measurement below. */
    private static ?array $functions = null;

    /**
     * Every function a query may name, by the short name it is written with.
     *
     * Built per request rather than cached: 239 reflections measured 0.08 ms,
     * against 210 ms for the query they are built for.
     *
     * @return array<string, \ReflectionFunction>
     */
    public static function functions(): array
    {
        if (self::$functions !== null) {
            return self::$functions;
        }

        $admitted = [];

        foreach (\get_defined_functions()['user'] as $name) {
            if (!\str_starts_with($name, 'flow\etl\dsl\\')) {
                continue;
            }

            $function = new \ReflectionFunction($name);
            $short = $function->getShortName();

            if (self::declined($short) !== null || self::takesACallable($function)) {
                continue;
            }

            if (self::admits((string) ($function->getReturnType() ?? ''))) {
                $admitted[$short] = $function;
            }
        }

        return self::$functions = $admitted;
    }

    public static function has(string $name): bool
    {
        return isset(self::functions()[$name]);
    }

    public static function declined(string $name): ?string
    {
        return self::DECLINED[$name] ?? null;
    }

    public static function function(string $name): ?\ReflectionFunction
    {
        return self::functions()[$name] ?? null;
    }

    /** The names a "did you mean" may suggest, and what an error lists. */
    public static function names(): array
    {
        $names = \array_keys(self::functions());
        \sort($names);

        return $names;
    }

    /**
     * How a function is written, for a message somebody reads.
     *
     * Rendered from the signature rather than from documentation, because the
     * signature is what the call will actually be checked against.
     */
    public static function signature(string $name): string
    {
        $function = self::function($name);

        if ($function === null) {
            return $name . '(...)';
        }

        $parameters = \array_map(static function (\ReflectionParameter $parameter): string {
            $type = (string) ($parameter->getType() ?? 'mixed');
            $written = ($parameter->isVariadic() ? '...' : '') . '$' . $parameter->getName();

            return $parameter->isOptional() && !$parameter->isVariadic()
                ? '[' . $type . ' ' . $written . ']'
                : $type . ' ' . $written;
        }, $function->getParameters());

        return $name . '(' . \implode(', ', $parameters) . ')';
    }

    /**
     * Whether a value type may be named by a query.
     *
     * Prefix matching on the written type, never a class load — see the class
     * docblock. A union or an intersection is refused: every admitted function
     * in Flow returns one type, and a union return would need a rule about
     * which half decides.
     */
    private static function admits(string $returnType): bool
    {
        $type = \ltrim($returnType, '?\\');

        if ($type === '' || \str_contains($type, '|') || \str_contains($type, '&')) {
            return false;
        }

        foreach (self::ADMITTED as $namespace) {
            if (\str_starts_with($type, $namespace)) {
                return true;
            }
        }

        return false;
    }

    /**
     * The one refusal that is about safety rather than about shape.
     *
     * A parameter typed `callable` or `Closure` is a place where a name from a
     * query would become a call, which is the whole thing this service is
     * built to prevent. Flow has two: `call()` and `to_callable()`.
     */
    private static function takesACallable(\ReflectionFunction $function): bool
    {
        foreach ($function->getParameters() as $parameter) {
            $type = (string) ($parameter->getType() ?? '');

            if (\stripos($type, 'callable') !== false || \str_contains($type, 'Closure')) {
                return true;
            }
        }

        return false;
    }
}
