<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * What a query may name, derived from Flow rather than listed by hand.
 *
 * ADR 0008's rule is that a name from the query never becomes a callable. That
 * rule is about **dispatch**, not about enumeration: a string that selects a
 * key in a map built here is exactly as safe as one that selects a `match`
 * arm, and it is what lets this file admit Flow's own function namespace
 * without widening the boundary by one call. The hand-written list it stands
 * beside admitted 37 names of the 239 Flow ships.
 *
 * ## Three rules decide admission, and none of them is a name
 *
 * **The return type's namespace.** A query composes values; it does not open
 * sources or write sinks. `Flow\ETL\Loader`, `Flow\ETL\Extractor` and
 * `Flow\ETL\Filesystem` are how a query would read or write a file of its own
 * choosing, so those categories are absent — the catalog owns what may be
 * read, and `write()` owns the three sinks that mean "return the rows".
 *
 * **Any parameter that accepts a callable.** `call()` and `to_callable()` are
 * the two Flow has, and they are precisely the remote code execution this
 * service exists to refuse. Refused by *signature* rather than by name, so a
 * function Flow adds later is refused before anybody here has heard of it.
 *
 * **[`Whitelist::DECLINED`]**, which is about correctness rather than safety
 * and keeps its own reasons.
 *
 * ## Why the return type is read as a string
 *
 * `class_exists()` and `is_a()` autoload, and `Flow\ETL\Function\Uuid` throws
 * at load when neither `ramsey/uuid` nor `symfony/uid` is installed. Loading a
 * class in order to decide whether a query may name it would take the service
 * down over an optional dependency of a function nobody called.
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

            if (Whitelist::declined($short) !== null || self::takesACallable($function)) {
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
