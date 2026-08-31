<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;

/**
 * Parse, build, run, and stop.
 *
 * The limits live here rather than in the language, because they are a property
 * of serving a request, not of what a query is allowed to say. A query written
 * for the CLI has no row cap; the same query through the panel does.
 */
final readonly class QueryRunner
{
    /**
     * How many rows one request may return.
     *
     * A cap rather than a stream: the panel renders a table, and a table with a
     * million rows is not a table. Hitting it is reported rather than hidden —
     * silently returning the first thousand of something is how people draw
     * conclusions from a slice they did not know was a slice.
     */
    public const int MAX_ROWS = 1_000;

    /** A simulation proves the transformation on a reviewable sample only. */
    public const int SIMULATION_ROWS = 25;

    /**
     * Wall-clock ceiling.
     *
     * A typical query measures in the low hundreds of milliseconds, so this is
     * a backstop against a query that walks far more than it meant to, not a
     * budget anyone should be near.
     */
    public const int TIMEOUT_SECONDS = 30;

    public function __construct(
        private Catalog $catalog,
        private string $source,
    ) {}

    /**
     * @param ?int $windowSeconds the panel's time window; null or zero reads everything
     *
     * @return array<string, mixed>
     */
    public function run(string $query, ?int $windowSeconds = null, int $maxRows = self::MAX_ROWS): array
    {
        $plan = (new PipelineBuilder($this->catalog, $windowSeconds))->build(Parser::parse($query));
        $maxRows = \max(1, \min(self::MAX_ROWS, $maxRows));

        \set_time_limit(self::TIMEOUT_SECONDS);
        $started = \microtime(true);

        // One more than the cap, so "there was more" is a fact rather than an
        // inference from a full page.
        $rows = $plan->frame->fetch($maxRows + 1)->toArray();
        $truncated = \count($rows) > $maxRows;

        if ($truncated) {
            $rows = \array_slice($rows, 0, $maxRows);
        }

        return [
            'columns' => $rows === [] ? [] : \array_keys($rows[0]),
            'rows' => $rows,
            'row_count' => \count($rows),
            'truncated' => $truncated,
            // From `to_output(truncate:)`: whether the panel may shorten cells.
            'truncate_cells' => $plan->truncate,
            'dataset' => $plan->dataset?->name,
            'grain' => $plan->dataset?->grain,
            'source' => $this->source,
            // What the rows were read through, so a table that looks short can
            // be read as scoped rather than as empty.
            'window_seconds' => $plan->windowSeconds,
            'took_ms' => (int) \round((\microtime(true) - $started) * 1000),
        ];
    }

    /**
     * The schemas, for the editor.
     *
     * Writing a query against an undocumented shape is guesswork, so the panel
     * shows this next to the editor.
     *
     * @return array<string, mixed>
     */
    public function datasets(): array
    {
        $out = [];

        foreach ($this->catalog->all() as $dataset) {
            $out[] = [
                'name' => $dataset->name,
                'aliases' => $dataset->name === 'runs' ? ['default'] : [],
                'grain' => $dataset->grain,
                'description' => $dataset->description,
                'requires_run' => $dataset->requiresRun,
                'columns' => \array_map(
                    static fn(string $name, string $type): array => ['name' => $name, 'type' => $type],
                    \array_keys($dataset->columns),
                    \array_values($dataset->columns),
                ),
            ];
        }

        return ['datasets' => $out, 'source' => $this->source, 'max_rows' => self::MAX_ROWS];
    }
}
