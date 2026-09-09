<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * The one rule that decides what a query may reach, on any of the three surfaces.
 *
 * There are three: the functions a query may name ([`Registry`]), the steps a
 * pipeline may take ([`Frame`]), and the methods a value supports
 * ([`Values`]). They used to be answered by three hand-written lists and are
 * now answered here, by signature — because a list is a queue: Flow ships 239
 * functions, 53 `DataFrame` methods and well over a hundred fluent methods on
 * a reference, and every one somebody forgot to type out was a thing this
 * query language could not say for no reason anybody could defend.
 *
 * ## What is refused, and why it is a *type* rather than a name
 *
 * A parameter that accepts a **callable** is where a name from a query would
 * become code — `call()` and `to_callable()` are the two Flow has, and any
 * third one Flow adds is refused before anybody here has heard of it.
 *
 * A parameter that accepts a **Loader, an Extractor, a Path or a Filesystem**
 * is where a query would open a source or write a sink of its own choosing.
 * The catalog decides what may be read; `write()` decides what "give the rows
 * back" means. `to_csv('/etc/anything')` in a service with no authentication
 * is a file write whose name looks as harmless as any other.
 *
 * A parameter that accepts a **Transformer, a Transformation or a
 * DataFrameFactory** is an object whose whole purpose is to run somebody's
 * code over the rows, which is the same hole with a longer name.
 *
 * Everything else composes values or reshapes a frame, and is admitted.
 *
 * ## What this does not do
 *
 * It does not load a class to decide. `class_exists()` autoloads, and
 * `Flow\ETL\Function\Uuid` throws at load without `ramsey/uuid` — deciding
 * admission by loading would take the service down over an optional
 * dependency of a function nobody called. Types are matched as **strings**.
 */
final class Admission
{
    /**
     * Types that make a parameter untouchable, by short name, and the reason.
     *
     * Matched against the **short name** of every type in a union rather than
     * as a substring of the whole: `Flow\ETL\Row` must be refused and
     * `Flow\ETL\Row\Reference` must not, and a substring rule cannot tell
     * those apart — it would refuse every method that takes a column.
     */
    public const array REFUSED = [
        'callable' => 'it takes a callable, which is where a name from a query would become code',
        'Closure' => 'it takes a closure, which is where a name from a query would become code',
        'Loader' => 'it writes rows somewhere, and where rows go is write()\'s decision rather than a query\'s',
        'Extractor' => 'it opens a source, and what may be read is the catalog\'s decision rather than a query\'s',
        'Path' => 'it takes a path, and this service must not be told which files to touch',
        'Filter' => 'it filters paths, which is a question about files rather than about rows',
        'Transformer' => 'it runs arbitrary code over the rows',
        'Transformation' => 'it runs arbitrary code over the rows',
        'Transformations' => 'it runs arbitrary code over the rows',
        'WithEntry' => 'it transforms entries through an object a query cannot build',
        'DataFrameFactory' => 'it builds frames from code rather than from the catalog',
        'SaveMode' => 'it is about writing files, which a query does not do',
        'ErrorHandler' => 'it changes what happens to failures, which is the service\'s decision',
        'SchemaValidator' => 'it is part of writing, not of asking',
        'Comparator' => 'it orders entries by an object a query cannot build',
        'Constraint' => 'it is a write-time check',
        'Formatter' => 'it renders for a console this service does not have',
        // Flow's own machinery. A query composes a description of work; it
        // does not reach into the run that will perform it.
        'Row' => 'it is Flow\'s own evaluation machinery rather than something a query composes',
        'Rows' => 'it is Flow\'s own evaluation machinery rather than something a query composes',
        'FlowContext' => 'it is Flow\'s own evaluation machinery rather than something a query composes',
        'EntryFactory' => 'it is Flow\'s own evaluation machinery rather than something a query composes',
        'Cache' => 'it is Flow\'s own storage rather than something a query composes',
    ];

    /**
     * Namespaces no parameter may come from, whatever the class is called.
     *
     * The short-name rule above is precise and therefore blind to a class
     * somebody renames; these catch the two areas where a rename must not
     * quietly open a door.
     */
    public const array REFUSED_NAMESPACES = [
        'Flow\Filesystem' => 'it reaches the filesystem',
        'Flow\ETL\Filesystem' => 'it reaches the filesystem',
        'Flow\ETL\Adapter' => 'it is an adapter to something outside this process',
        'Flow\ETL\Transformer' => 'it runs arbitrary code over the rows',
    ];

    /**
     * Why a call is refused, or null when it is not.
     *
     * The reason is returned rather than a boolean because it is what the
     * error message says: "not part of the query language" sends somebody
     * looking for a typo when the truth is that the thing exists and is
     * deliberately out of reach.
     */
    public static function refuses(\ReflectionFunctionAbstract $callable): ?string
    {
        foreach ($callable->getParameters() as $parameter) {
            foreach (self::types($parameter) as $type) {
                $short = \strrchr($type, '\\');
                $short = $short === false ? $type : \substr($short, 1);

                if (isset(self::REFUSED[$short])) {
                    return self::REFUSED[$short];
                }

                foreach (self::REFUSED_NAMESPACES as $namespace => $reason) {
                    if (\str_starts_with($type, $namespace . '\\')) {
                        return $reason;
                    }
                }
            }
        }

        return null;
    }

    /**
     * Every type a parameter accepts, one at a time.
     *
     * A union is refused if *any* half is, because the caller picks which half
     * it passes.
     *
     * @return list<string>
     */
    private static function types(\ReflectionParameter $parameter): array
    {
        $written = (string) ($parameter->getType() ?? 'mixed');
        $parts = \preg_split('/[|&]/', $written);

        return \array_map(
            static fn(string $type): string => \ltrim($type, '?\\'),
            $parts === false ? [$written] : $parts,
        );
    }

    /**
     * What a callable's return type names, with `self`, `static` and `parent`
     * resolved to the class they stand for.
     *
     * A relative type is the one thing reflection does not report the same way
     * on every PHP this service supports. Flow declares almost every
     * `DataFrame` method `: self`; PHP 8.5 resolves that to
     * `Flow\ETL\DataFrame` in `ReflectionNamedType::getName()` and 8.3 and 8.4
     * report the word `self`. Matching the written word therefore admitted 24
     * steps on one interpreter and one on another — the same source, the same
     * lock file, two different query languages, and the difference only visible
     * to whoever ran the suite on the version CI does not.
     *
     * Resolving here rather than at each call site is the point: [`Frame`] and
     * [`Values`] ask the same question and must not answer it twice.
     */
    public static function returned(\ReflectionFunctionAbstract $callable): string
    {
        $type = $callable->getReturnType();

        if (!$type instanceof \ReflectionNamedType) {
            // A union or an intersection, which `returns()` refuses anyway.
            return (string) ($type ?? '');
        }

        $written = $type->getName();
        $class = $callable instanceof \ReflectionMethod ? $callable->getDeclaringClass() : null;

        if ($class === null) {
            return $written;
        }

        $parent = $class->getParentClass();

        return match (\strtolower($written)) {
            'self', 'static' => $class->getName(),
            'parent' => ($parent === false ? $class : $parent)->getName(),
            default => $written,
        };
    }

    /**
     * Whether a written return type sits in one of the admitted namespaces.
     *
     * A union or an intersection is refused: every admitted member of Flow's
     * API returns one type, and a union return would need a rule about which
     * half decides.
     *
     * Callers hand this [`returned()`]'s answer rather than the written type,
     * so that a relative name has already become the class it stands for.
     *
     * @param list<string> $namespaces
     */
    public static function returns(string $type, array $namespaces): bool
    {
        $type = \ltrim($type, '?\\');

        if ($type === '' || \str_contains($type, '|') || \str_contains($type, '&')) {
            return false;
        }

        foreach ($namespaces as $namespace) {
            if (\str_starts_with($type, $namespace)) {
                return true;
            }
        }

        return false;
    }

    /**
     * How a call is written, for a message somebody reads.
     *
     * Rendered from the signature rather than from documentation, because the
     * signature is what the call will actually be checked against.
     */
    public static function signature(string $name, \ReflectionFunctionAbstract $callable): string
    {
        $parameters = \array_map(static function (\ReflectionParameter $parameter): string {
            $type = (string) ($parameter->getType() ?? 'mixed');
            $written = ($parameter->isVariadic() ? '...' : '') . '$' . $parameter->getName();

            return $parameter->isOptional() && !$parameter->isVariadic()
                ? '[' . $type . ' ' . $written . ']'
                : $type . ' ' . $written;
        }, $callable->getParameters());

        return $name . '(' . \implode(', ', $parameters) . ')';
    }
}
