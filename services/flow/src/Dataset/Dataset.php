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
     * @param array<string, string> $columns column name => type, for /flow/datasets
     * @param array<string, string> $hints   column name => what to write instead, for near-misses
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
    ) {}

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
