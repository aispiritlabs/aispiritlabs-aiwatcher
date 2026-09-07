<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dataset\Dataset;
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
        private ?ExecutionMemory $memory = null,
    ) {}

    /**
     * @param ?int              $windowSeconds the panel's time window; null or zero reads everything
     * @param ?string           $executionId   `<execution>/<step>/<attempt>` for a managed run, null for an ad-hoc one
     * @param ?array{int, int}  $windowSpan    exact bounds a managed plan pinned, when it pinned any
     *
     * @return array<string, mixed>
     */
    public function run(
        string $query,
        ?int $windowSeconds = null,
        int $maxRows = self::MAX_ROWS,
        ?string $executionId = null,
        ?array $windowSpan = null,
    ): array {
        // The span's end, when a managed plan pinned one. Its *width* still
        // comes from `window_seconds` or from the script's own `period:`, so a
        // query that pins both keeps the narrower of the two.
        $plan = (new PipelineBuilder($this->catalog, $windowSeconds, $windowSpan[1] ?? null))->build(Parser::parse(
            $query,
        ));
        $maxRows = \max(1, \min(self::MAX_ROWS, $maxRows));

        \set_time_limit(self::TIMEOUT_SECONDS);
        $started = \microtime(true);

        // Only a managed run is remembered. An ad-hoc query from the Query tab is not
        // keyed, is not resumed and is not deduplicated — ADR 0008's shape, unchanged —
        // so there is nothing to write a note about.
        $note = $executionId === null ? null : $this->memory;

        // Noted before the rows are fetched, because the whole value of `running` is that
        // it is true *while* the query executes — which is the window a reactor's timeout
        // falls in. A query that then throws clears the note in the `catch`, so a failure
        // leaves `absent` rather than a `running` nothing will ever complete.
        $note?->started((string) $executionId);

        try {
            // One more than the cap, so "there was more" is a fact rather than an
            // inference from a full page.
            $rows = $plan->frame->fetch($maxRows + 1)->toArray();
        } catch (\Throwable $error) {
            $note?->failed((string) $executionId);

            throw $error;
        }
        $truncated = \count($rows) > $maxRows;

        if ($truncated) {
            $rows = \array_slice($rows, 0, $maxRows);
        }

        $digest = ExecutionMemory::digestOf($rows);
        $note?->finished((string) $executionId, $digest, \count($rows));

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
            // Which of the two the rows were actually read through. Declared
            // rather than left for the caller to infer: only this service knows
            // what its sources accept, and a managed plan that pinned
            // 09:00–10:00 needs to know whether it got that or a width applied
            // from now. See `windowApplied`.
            'window_applied' => self::windowApplied($windowSpan, $plan->dataset),
            'took_ms' => (int) \round((\microtime(true) - $started) * 1000),
            // What this result hashed to, in *this* service's encoding. A fingerprint for
            // comparing two runs of one query, never a key into anybody's store — see
            // `ExecutionMemory::digestOf`.
            'digest' => $digest,
        ];
    }

    /**
     * Whether the rows were read through exact bounds or through a width.
     *
     * `duration`, always, today — and the reason is one layer down rather than
     * here. Flow reads its datasets from the aiwatcher API, whose windowed
     * routes take `window_seconds` and apply it from their own now; a span
     * cannot be expressed to them, so a span asked for here is narrowed to the
     * width it is worth.
     *
     * The field exists so that the narrowing is a *fact the caller receives*
     * rather than something it has to know. A managed run reads it and, today,
     * declines to cache a windowed step because of it. When the API's list
     * routes accept bounds, this function returns `span` for a request that
     * carried them, and that cache condition opens with no change on the Rust
     * side — which is what makes this an extension point rather than a
     * limitation nobody wrote down.
     *
     * @param ?array{int, int} $windowSpan
     */
    private static function windowApplied(?array $windowSpan, ?Dataset $dataset): string
    {
        if ($windowSpan === null) {
            return 'none';
        }

        // A dataset that takes no window cannot be read through one, and saying
        // `span` about it would be a claim nobody could act on. `runs` and
        // `spans` are the two that can; `events` is scoped by its run.
        return $dataset !== null && $dataset->windowed ? 'span' : 'none';
    }

    /**
     * What this service remembers about a managed execution, if anything.
     *
     * The lookup half of section 15.4. `absent` is the ordinary answer and the safe one:
     * the reactor treats it as "run it again", and a retry of a deterministic query over
     * the same window writes the same digest. A second replica behind a load balancer
     * therefore answers `absent` for the other replica's execution, by design.
     *
     * @return array{state: string, digest?: string, rows?: int}
     */
    public function seen(string $executionId): array
    {
        return $this->memory?->seen($executionId) ?? ['state' => ExecutionMemory::ABSENT];
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
                // What a read() may carry beyond the dataset name. Listed for
                // the same reason the columns are: the API rejects unknown
                // query parameters, so this is the contract rather than a hint.
                'parameters' => \array_values(\array_map(static fn($parameter): array => [
                    'name' => $parameter->name,
                    'required' => $parameter->required,
                    'description' => $parameter->description,
                    'values' => $parameter->values,
                ], $dataset->parameters)),
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
