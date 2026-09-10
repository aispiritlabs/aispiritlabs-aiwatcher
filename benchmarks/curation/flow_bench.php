<?php

declare(strict_types=1);

/**
 * Flow PHP's half of the benchmark: one query, one process, one JSON line out.
 *
 * Each query is the twin of one in `polars_bench.py`, written with the same
 * DataFrame vocabulary the Query tab admits — `filter`, `withEntry`, `join`,
 * `groupBy`/`aggregate` — so a time measured here is the time that language
 * costs, not the time of a hand-rolled PHP loop nobody would ship.
 *
 *   php flow_bench.php --query q2 --corpus .data/10GB --out results/outputs/flow
 */

require __DIR__ . '/vendor/autoload.php';

use Flow\ETL\DataFrame;
use Flow\ETL\Extractor;
use Flow\ETL\Join\Join;
use Flow\ETL\Schema;

use function Flow\ETL\Adapter\CSV\from_csv;
use function Flow\ETL\Adapter\CSV\to_csv;
use function Flow\ETL\Adapter\Parquet\from_parquet;
use function Flow\ETL\DSL\average;
use function Flow\ETL\DSL\count;
use function Flow\ETL\DSL\data_frame;
use function Flow\ETL\DSL\float_schema;
use function Flow\ETL\DSL\int_schema;
use function Flow\ETL\DSL\join_on;
use function Flow\ETL\DSL\lit;
use function Flow\ETL\DSL\max as flow_max;
use function Flow\ETL\DSL\ref;
use function Flow\ETL\DSL\schema;
use function Flow\ETL\DSL\str_schema;
use function Flow\ETL\DSL\sum;

/** `corpus.py`'s SPANS, in Flow's terms: typed on read, never inferred. */
function spans_schema(): Schema
{
    return schema(
        str_schema('run_id'),
        str_schema('trace_id'),
        str_schema('span_id'),
        str_schema('parent_span_id', nullable: true),
        str_schema('name'),
        str_schema('kind'),
        str_schema('start'),
        str_schema('end'),
        int_schema('duration_ms'),
        str_schema('operation', nullable: true),
        str_schema('agent_id', nullable: true),
        str_schema('model', nullable: true),
        str_schema('tool', nullable: true),
        str_schema('step_type', nullable: true),
        str_schema('status'),
        int_schema('input_tokens', nullable: true),
        int_schema('output_tokens', nullable: true),
        str_schema('error', nullable: true),
    );
}

/**
 * The spans, from either layout.
 *
 * Parquet is handed the columns its query reads, because that is what a Flow
 * user writes and it is Parquet's whole advantage; Polars works the same
 * projection out for itself. CSV has no such thing — every byte is parsed.
 *
 * @param list<string> $columns
 */
function spans(string $corpus, string $format, array $columns, int $batchSize): DataFrame
{
    $extractor = match ($format) {
        'csv' => from_csv("{$corpus}/spans.csv/part-*.csv", schema: spans_schema()),
        'parquet' => from_parquet("{$corpus}/spans.parquet/part-*.parquet", columns: $columns),
    };

    return data_frame()->read($extractor)->batchSize($batchSize);
}

function prices(string $corpus): Extractor
{
    return from_csv(
        "{$corpus}/models.csv",
        schema: schema(
            str_schema('model'),
            str_schema('provider'),
            float_schema('usd_per_input_token'),
            float_schema('usd_per_output_token'),
        ),
    );
}

/** @return array<string, mixed> */
function q1(string $corpus, string $format, string $out, int $batchSize): array
{
    $count = spans($corpus, $format, ['status', 'duration_ms'], $batchSize)
        ->filter(ref('status')->same(lit('error'))->and(ref('duration_ms')->greaterThan(lit(1000))))
        ->count();

    return ['count' => $count];
}

/** @return list<array<string, mixed>> */
function q2(string $corpus, string $format, string $out, int $batchSize): array
{
    return spans($corpus, $format, ['kind', 'model', 'span_id', 'input_tokens', 'output_tokens', 'duration_ms'], $batchSize)
        ->filter(ref('kind')->same(lit('llm')))
        ->groupBy(ref('model'))
        ->aggregate(
            count(ref('span_id')),
            sum(ref('input_tokens')),
            sum(ref('output_tokens')),
            // Flow's own default is two decimal places; asked for more so the
            // comparison is with Polars' mean rather than with a rounding rule.
            average(ref('duration_ms'), scale: 6),
        )
        ->rename('span_id_count', 'spans')
        ->rename('input_tokens_sum', 'input_tokens')
        ->rename('output_tokens_sum', 'output_tokens')
        ->rename('duration_ms_avg', 'avg_duration_ms')
        ->fetch()
        ->toArray();
}

/** @return array<string, mixed> */
function q3(string $corpus, string $format, string $out, int $batchSize): array
{
    $target = "{$out}/q3.csv";
    @\unlink($target);

    spans($corpus, $format, ['run_id', 'span_id', 'input_tokens', 'output_tokens', 'duration_ms'], $batchSize)
        ->groupBy(ref('run_id'))
        ->aggregate(
            count(ref('span_id')),
            sum(ref('input_tokens')),
            sum(ref('output_tokens')),
            flow_max(ref('duration_ms')),
        )
        ->rename('span_id_count', 'spans')
        ->rename('input_tokens_sum', 'input_tokens')
        ->rename('output_tokens_sum', 'output_tokens')
        ->rename('duration_ms_max', 'max_duration_ms')
        ->write(to_csv($target))
        ->run();

    return ['output' => $target];
}

/** @return array<string, mixed> */
function q4(string $corpus, string $format, string $out, int $batchSize): array
{
    $target = "{$out}/q4.csv";
    @\unlink($target);

    $columns = ['span_id', 'run_id', 'start', 'agent_id', 'model', 'status', 'kind', 'input_tokens', 'output_tokens', 'duration_ms'];

    spans($corpus, $format, $columns, $batchSize)
        ->filter(ref('status')->same(lit('ok'))->and(ref('kind')->same(lit('llm'))))
        ->join(data_frame()->read(prices($corpus)), join_on(['model' => 'model'], 'price_'), Join::inner)
        ->withEntry('day', ref('start')->stringBefore('T'))
        ->withEntry('provider', ref('price_provider'))
        ->withEntry('total_tokens', ref('input_tokens')->plus(ref('output_tokens')))
        ->withEntry(
            'cost_usd',
            ref('input_tokens')
                ->multiply(ref('price_usd_per_input_token'))
                ->plus(ref('output_tokens')->multiply(ref('price_usd_per_output_token'))),
        )
        ->select('span_id', 'run_id', 'day', 'agent_id', 'model', 'provider', 'total_tokens', 'cost_usd', 'duration_ms')
        ->write(to_csv($target))
        ->run();

    return ['output' => $target];
}

$options = \getopt('', ['query:', 'corpus:', 'format:', 'out:', 'batch-size:']);
$query = (string) ($options['query'] ?? '');
$corpus = \rtrim((string) ($options['corpus'] ?? ''), '/');
$format = (string) ($options['format'] ?? 'csv');
$out = \rtrim((string) ($options['out'] ?? ''), '/');
// Flow's own default, from `BatchSizeOptimization`; the harness can move it.
$batchSize = (int) ($options['batch-size'] ?? 1000);

if (!\in_array($query, ['q1', 'q2', 'q3', 'q4'], true) || $corpus === '' || $out === '' || !\in_array($format, ['csv', 'parquet'], true)) {
    \fwrite(\STDERR, "usage: php flow_bench.php --query q1|q2|q3|q4 --corpus DIR --out DIR [--format csv|parquet] [--batch-size N]\n");
    exit(2);
}

if (!\is_dir($out)) {
    \mkdir($out, recursive: true);
}

$started = \hrtime(true);
$result = $query($corpus, $format, $out, $batchSize);
$seconds = (\hrtime(true) - $started) / 1e9;

$jit = \function_exists('opcache_get_status') ? (\opcache_get_status(false)['jit']['on'] ?? false) : false;

echo \json_encode([
    'engine' => 'flow',
    'query' => $query,
    'format' => $format,
    'seconds' => \round($seconds, 3),
    'batch_size' => $batchSize,
    'php' => \PHP_VERSION,
    'jit' => $jit,
    'peak_memory_bytes' => \memory_get_peak_usage(true),
    'result' => $result,
], \JSON_THROW_ON_ERROR), "\n";
