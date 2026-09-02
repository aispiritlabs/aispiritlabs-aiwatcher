<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

/**
 * One named thing a query can read.
 *
 * A dataset is an aiwatcher API route plus enough description to turn its
 * responses into rows: where the rows sit in the body, which query parameter
 * replays the cursor, and what columns come out.
 *
 * The column list is not documentation — it is the contract. `Catalog` projects
 * exactly these columns and the parser rejects references to anything else, so
 * what `/flow/datasets` shows is what a query can actually use. A schema that
 * can drift from the data is a schema nobody trusts.
 */
final readonly class Dataset
{
    /**
     * @param array<string, string>    $columns    column name => type, for /flow/datasets
     * @param array<string, string>    $hints      column name => what to write instead, for near-misses
     * @param array<string, Parameter> $parameters named read() arguments this route accepts
     */
    public function __construct(
        public string $name,
        public string $path,
        /** Where the array of rows sits in the response body. */
        public string $rowsPath,
        /** The query parameter a cursor is replayed as. */
        public string $cursorParam,
        public string $grain,
        public string $description,
        public array $columns,
        public array $hints = [],
        /**
         * Whether the route addresses a single run.
         *
         * `events` is per-run: without a run there is no route to call, and
         * walking every run instead would be an N+1 across the whole retention
         * window dressed up as one query.
         */
        public bool $requiresRun = false,
        /**
         * Whether the route accepts `window_seconds`.
         *
         * The aiwatcher API rejects unknown query parameters rather than
         * ignoring them, so this is not a hint — sending the window to a route
         * that has none turns the whole query into a 400. `events` is per-run
         * and has no window: a run's log is bounded by the run.
         */
        public bool $windowed = false,
        /**
         * Named arguments `read()` may carry, beyond `run:` and `period:`.
         *
         * The hub search takes a query string; the annotation list takes a
         * project. Both are ordinary query parameters on the API, and both are
         * useless as a filter applied after the fact — reading every row to
         * throw most of them away is the thing the catalog exists to avoid.
         */
        public array $parameters = [],
    ) {}

    /** What to tell someone who wrote an argument this route has no idea about. */
    public function explainUnknownParameter(string $name): string
    {
        $declared = \array_keys($this->parameters);
        $available = $declared === []
            ? 'It takes a dataset name, and period: when it is windowed.'
            : \sprintf('It takes: %s.', \implode(', ', \array_map(
                static fn(string $key): string => $key . ':',
                $declared,
            )));

        return \sprintf('Dataset "%s" has no read() argument "%s". %s', $this->name, $name, $available);
    }

    public function hasColumn(string $name): bool
    {
        return \array_key_exists($name, $this->columns);
    }

    /**
     * What to tell someone who referenced a column that does not exist.
     *
     * An explicit hint when there is one, then a nearest-name guess, then the
     * full list. "Unknown column" on its own sends people to the docs; the
     * catalog already knows the answer, so it should say it.
     */
    public function explainUnknownColumn(string $name): string
    {
        if (isset($this->hints[$name])) {
            return \sprintf('Dataset "%s" has no column "%s". %s', $this->name, $name, $this->hints[$name]);
        }

        $closest = null;
        $distance = \PHP_INT_MAX;

        foreach (\array_keys($this->columns) as $known) {
            $candidate = \levenshtein($name, $known);

            if ($candidate < $distance) {
                $distance = $candidate;
                $closest = $known;
            }
        }

        $suggestion = $closest !== null && $distance <= 3 ? \sprintf(' Did you mean "%s"?', $closest) : '';

        return \sprintf(
            'Dataset "%s" has no column "%s".%s Available: %s.',
            $this->name,
            $name,
            $suggestion,
            \implode(', ', \array_keys($this->columns)),
        );
    }
}
