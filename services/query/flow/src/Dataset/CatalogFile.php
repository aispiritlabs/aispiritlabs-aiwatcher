<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

/**
 * The catalog every query engine serves, read from the one file that declares it.
 *
 * `services/query/contract/catalog.json` holds what a dataset *is* — its route, where its
 * rows sit in a response, which parameter replays its cursor, its columns, its hints and
 * its `read()` arguments — and every engine loads it: Flow here, DataFusion and DuckDB
 * from the same bytes (AW-3). How a dataset is *read* stays in each engine, because that
 * is the half written in each engine's own language.
 *
 * Read strictly. A key this class does not know is refused naming the dataset and the
 * key, because the likeliest one is a near miss, and `"window": true` quietly leaving
 * `windowed` false is a query that sends its window nowhere and reads everything.
 * `$comment` is the one key every level skips: JSON has no comments, and some of these
 * declarations need their reason beside them.
 */
final class CatalogFile
{
    private const array DATASET_KEYS = [
        'name',
        'grain',
        'description',
        'path',
        'rows_path',
        'cursor_param',
        'requires_run',
        'windowed',
        'columns',
        'hints',
        'parameters',
        'corpus',
    ];

    private const array PARAMETER_KEYS = ['name', 'required', 'description', 'values'];

    /** @var array<string, array<string, Dataset>> */
    private static array $read = [];

    /**
     * Where the file is: beside this engine in the repository, and at `/contract` in the
     * image, which `deploy/Dockerfile.flow` copies it to. `AIWATCHER_QUERY_CATALOG` wins.
     */
    public static function defaultPath(): string
    {
        $configured = \getenv('AIWATCHER_QUERY_CATALOG');

        return \is_string($configured) && $configured !== ''
            ? $configured
            : \rtrim(\dirname(__DIR__, 3), '/') . '/contract/catalog.json';
    }

    /**
     * Every dataset the file declares, by name, in the order it declares them.
     *
     * Read once per process: the file is part of the build, not of a request.
     *
     * @return array<string, Dataset>
     */
    public static function read(string $path): array
    {
        return self::$read[$path] ??= self::parse($path);
    }

    /** @return array<string, Dataset> */
    private static function parse(string $path): array
    {
        $raw = \is_file($path) ? \file_get_contents($path) : false;

        if ($raw === false) {
            throw new \RuntimeException(\sprintf(
                'The query catalog is not at %s. Set AIWATCHER_QUERY_CATALOG to where it is.',
                $path,
            ));
        }

        try {
            $document = \json_decode($raw, true, 32, \JSON_THROW_ON_ERROR);
        } catch (\JsonException $error) {
            throw new \RuntimeException(
                \sprintf('The query catalog at %s is not JSON: %s', $path, $error->getMessage()),
                0,
                $error,
            );
        }

        if (
            !\is_array($document)
            || ($document['version'] ?? null) !== 1
            || !\is_array($document['datasets'] ?? null)
        ) {
            throw new \RuntimeException(\sprintf(
                'The query catalog at %s is not version 1 with a list of datasets.',
                $path,
            ));
        }

        self::refuseUnknown($document, ['version', 'datasets'], \sprintf('the catalog at %s', $path));

        $datasets = [];

        foreach ($document['datasets'] as $entry) {
            $dataset = self::dataset($entry, $path);

            if (isset($datasets[$dataset->name])) {
                throw new \RuntimeException(\sprintf('%s declares "%s" twice.', $path, $dataset->name));
            }

            $datasets[$dataset->name] = $dataset;
        }

        return $datasets;
    }

    private static function dataset(mixed $entry, string $path): Dataset
    {
        if (!\is_array($entry) || !\is_string($entry['name'] ?? null)) {
            throw new \RuntimeException(\sprintf('A dataset in %s has no name.', $path));
        }

        $where = \sprintf('dataset "%s" in %s', $entry['name'], $path);
        self::refuseUnknown($entry, self::DATASET_KEYS, $where);

        return new Dataset(
            name: $entry['name'],
            path: self::text($entry, 'path', $where, ''),
            rowsPath: self::text($entry, 'rows_path', $where, ''),
            cursorParam: self::text($entry, 'cursor_param', $where, ''),
            grain: self::text($entry, 'grain', $where),
            description: self::text($entry, 'description', $where),
            columns: self::table($entry, 'columns', $where),
            hints: self::table($entry, 'hints', $where),
            requiresRun: self::flag($entry, 'requires_run', $where),
            windowed: self::flag($entry, 'windowed', $where),
            parameters: self::parameters($entry, $where),
            corpus: isset($entry['corpus']) ? self::text($entry, 'corpus', $where) : null,
        );
    }

    /**
     * @param array<array-key, mixed> $entry
     *
     * @return array<string, Parameter>
     */
    private static function parameters(array $entry, string $where): array
    {
        $declared = $entry['parameters'] ?? [];

        if (!\is_array($declared)) {
            throw new \RuntimeException(\sprintf('%s: "parameters" is a list.', $where));
        }

        $parameters = [];

        foreach ($declared as $one) {
            if (!\is_array($one) || !\is_string($one['name'] ?? null)) {
                throw new \RuntimeException(\sprintf('%s: a parameter has no name.', $where));
            }

            $at = \sprintf('parameter "%s" of %s', $one['name'], $where);
            self::refuseUnknown($one, self::PARAMETER_KEYS, $at);
            $values = $one['values'] ?? [];

            if (
                !\is_array($values)
                || !\array_is_list($values)
                || \array_filter($values, \is_string(...)) !== $values
            ) {
                throw new \RuntimeException(\sprintf('%s: "values" is a list of strings.', $at));
            }

            $parameters[$one['name']] = new Parameter(
                name: $one['name'],
                required: self::flag($one, 'required', $at),
                description: self::text($one, 'description', $at),
                values: $values,
            );
        }

        return $parameters;
    }

    /** @param array<array-key, mixed> $entry */
    private static function text(array $entry, string $key, string $where, ?string $default = null): string
    {
        $value = $entry[$key] ?? $default;

        if (!\is_string($value)) {
            throw new \RuntimeException(\sprintf('%s: "%s" is a string, and it is required.', $where, $key));
        }

        return $value;
    }

    /** @param array<array-key, mixed> $entry */
    private static function flag(array $entry, string $key, string $where): bool
    {
        $value = $entry[$key] ?? false;

        if (!\is_bool($value)) {
            throw new \RuntimeException(\sprintf('%s: "%s" is true or false.', $where, $key));
        }

        return $value;
    }

    /**
     * A name-to-text object — columns to their types, or names to hints.
     *
     * @param array<array-key, mixed> $entry
     *
     * @return array<string, string>
     */
    private static function table(array $entry, string $key, string $where): array
    {
        $value = $entry[$key] ?? [];

        if (!\is_array($value)) {
            throw new \RuntimeException(\sprintf('%s: "%s" maps names to text.', $where, $key));
        }

        $table = [];

        foreach ($value as $name => $text) {
            if ($name === '$comment') {
                continue;
            }

            if (!\is_string($name) || !\is_string($text)) {
                throw new \RuntimeException(\sprintf('%s: "%s" maps names to text.', $where, $key));
            }

            $table[$name] = $text;
        }

        return $table;
    }

    /**
     * @param array<array-key, mixed> $entry
     * @param list<string>            $known
     */
    private static function refuseUnknown(array $entry, array $known, string $where): void
    {
        foreach (\array_keys($entry) as $key) {
            if ($key === '$comment' || \in_array($key, $known, true)) {
                continue;
            }

            throw new \RuntimeException(\sprintf(
                '%s has a key "%s" this catalog does not know. The keys are: %s.',
                $where,
                (string) $key,
                \implode(', ', $known),
            ));
        }
    }
}
