<?php

declare(strict_types=1);

/**
 * Where Flow's time goes: q1 taken apart stage by stage, over one corpus.
 *
 * Each line adds one thing to the line above it — reading lines, splitting
 * fields, Flow's rows, Flow's types, the predicate — so the difference between
 * two lines is what that one thing costs. The knobs a Flow user can turn
 * (batch size, which is the memory knob; an early `select`; the collector;
 * Parquet with a projection) are measured against the same baseline.
 *
 *   php -d memory_limit=8G -d opcache.enable_cli=1 -d opcache.jit=tracing \
 *       -d opcache.jit_buffer_size=128M profile_flow.php .data/100MB
 */

require __DIR__ . '/vendor/autoload.php';

use Flow\ETL\DataFrame;
use Flow\ETL\Join\Join;

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
use function Flow\ETL\DSL\sum;
use function Flow\ETL\DSL\ref;
use function Flow\ETL\DSL\schema;
use function Flow\ETL\DSL\str_schema;

$corpus = \rtrim($argv[1] ?? __DIR__ . '/.data/100MB', '/');
$only = $argv[2] ?? null;
$files = \glob("{$corpus}/spans.csv/part-*.csv") ?: [];
$csv = "{$corpus}/spans.csv/part-*.csv";

/** The q1 predicate's two columns, by position in the file. */
const DURATION = 8;
const STATUS = 14;

function spans_schema(): Flow\ETL\Schema
{
    return schema(
        str_schema('run_id'), str_schema('trace_id'), str_schema('span_id'),
        str_schema('parent_span_id', nullable: true), str_schema('name'), str_schema('kind'),
        str_schema('start'), str_schema('end'), int_schema('duration_ms'),
        str_schema('operation', nullable: true), str_schema('agent_id', nullable: true),
        str_schema('model', nullable: true), str_schema('tool', nullable: true),
        str_schema('step_type', nullable: true), str_schema('status'),
        int_schema('input_tokens', nullable: true), int_schema('output_tokens', nullable: true),
        str_schema('error', nullable: true),
    );
}

function q1(DataFrame $frame): int
{
    return $frame
        ->filter(ref('status')->same(lit('error'))->and(ref('duration_ms')->greaterThan(lit(1000))))
        ->count();
}

/** @param callable(): int $work */
function measure(string $label, callable $work, bool $collector = true): void
{
    global $only;
    if ($only !== null && !\str_contains($label, $only)) {
        return;
    }
    \gc_collect_cycles();
    $collector ? \gc_enable() : \gc_disable();
    $before = \gc_status();
    \memory_reset_peak_usage();
    $started = \hrtime(true);
    $result = $work();
    $seconds = (\hrtime(true) - $started) / 1e9;
    $after = \gc_status();
    \gc_enable();
    \printf(
        "%-60s %7.2f s  %9s  peak %6.1f MiB  gc runs %4d  gc time %5.2f s\n",
        $label,
        $seconds,
        \number_format($result),
        \memory_get_peak_usage(true) / 2 ** 20,
        $after['runs'] - $before['runs'],
        ($after['collector_time'] ?? 0) - ($before['collector_time'] ?? 0),
    );
}

\printf("corpus %s, %d parts, PHP %s, JIT %s\n\n", $corpus, \count($files), \PHP_VERSION,
    \function_exists('opcache_get_status') && (\opcache_get_status(false)['jit']['on'] ?? false) ? 'on' : 'off');

// ── PHP itself: the floor ─────────────────────────────────────────────────────

measure('PHP  fgets: every line', static function () use ($files): int {
    $lines = 0;
    foreach ($files as $file) {
        $handle = \fopen($file, 'r');
        while (\fgets($handle) !== false) {
            $lines++;
        }
        \fclose($handle);
    }
    return $lines;
});

measure('PHP  fgetcsv: every field', static function () use ($files): int {
    $rows = 0;
    foreach ($files as $file) {
        $handle = \fopen($file, 'r');
        while (\fgetcsv($handle, escape: '') !== false) {
            $rows++;
        }
        \fclose($handle);
    }
    return $rows;
});

measure('PHP  fgetcsv + the q1 predicate, a hand-written loop', static function () use ($files): int {
    $matches = 0;
    foreach ($files as $file) {
        $handle = \fopen($file, 'r');
        \fgetcsv($handle, escape: '');
        while (($row = \fgetcsv($handle, escape: '')) !== false) {
            if ($row[STATUS] === 'error' && (int) $row[DURATION] > 1000) {
                $matches++;
            }
        }
        \fclose($handle);
    }
    return $matches;
});

// ── Flow, one layer at a time ─────────────────────────────────────────────────

measure('Flow from_csv, untyped (every cell a string) → count()', static fn(): int
    => data_frame()->read(from_csv($csv))->count());

measure('Flow from_csv, typed by schema → count()', static fn(): int
    => data_frame()->read(from_csv($csv, schema: spans_schema()))->count());

measure('Flow q1: typed + filter → count()  [baseline]', static fn(): int
    => q1(data_frame()->read(from_csv($csv, schema: spans_schema()))));

// ── The knobs ─────────────────────────────────────────────────────────────────

foreach ([100, 10_000, 100_000] as $batch) {
    measure(\sprintf('Flow q1, batch %s rows (the memory knob)', \number_format($batch)), static fn(): int
        => q1(data_frame()->read(from_csv($csv, schema: spans_schema()))->batchSize($batch)));
}

measure('Flow q1, select the 2 columns straight after read', static fn(): int
    => q1(data_frame()->read(from_csv($csv, schema: spans_schema()))->select('status', 'duration_ms')));

measure('Flow q1, cycle collector off', static fn(): int
    => q1(data_frame()->read(from_csv($csv, schema: spans_schema()))), collector: false);

measure('Flow q1 over Parquet, 2 columns projected', static fn(): int
    => q1(data_frame()->read(from_parquet("{$corpus}/spans.parquet/part-*.parquet", columns: ['status', 'duration_ms']))));

measure('Flow q1 over Parquet, every column', static fn(): int
    => q1(data_frame()->read(from_parquet("{$corpus}/spans.parquet/part-*.parquet"))));

// ── q2 and q4: what the rest of a query costs ─────────────────────────────────

$typed = static fn(): DataFrame => data_frame()->read(from_csv($csv, schema: spans_schema()));
$llm = static fn(): DataFrame => $typed()->filter(ref('kind')->same(lit('llm')));
$ok = static fn(): DataFrame => $typed()->filter(ref('status')->same(lit('ok'))->and(ref('kind')->same(lit('llm'))));
$prices = static fn(): DataFrame => data_frame()->read(from_csv("{$corpus}/models.csv", schema: schema(
    str_schema('model'), str_schema('provider'),
    float_schema('usd_per_input_token'), float_schema('usd_per_output_token'),
)));
$joined = static fn(): DataFrame => $ok()->join($prices(), join_on(['model' => 'model'], 'price_'), Join::inner);

measure('q2   filter kind = llm → count()', static fn(): int => $llm()->count());
measure('q2   + groupBy(model), 4 aggregations  [q2]', static fn(): int => \count(
    $llm()->groupBy(ref('model'))->aggregate(
        count(ref('span_id')), sum(ref('input_tokens')), sum(ref('output_tokens')), average(ref('duration_ms')),
    )->fetch()->toArray(),
));

measure('q4   filter status = ok, kind = llm → count()', static fn(): int => $ok()->count());
measure('q4   + join the 8-row price table', static fn(): int => $joined()->count());
measure('q4   + join + 2 derived strings (day, provider)', static fn(): int => $joined()
    ->withEntry('day', ref('start')->stringBefore('T'))
    ->withEntry('provider', ref('price_provider'))
    ->count());
measure('q4   + join + 2 strings + 2 BigDecimal sums (total, cost)', static fn(): int => $joined()
    ->withEntry('day', ref('start')->stringBefore('T'))
    ->withEntry('provider', ref('price_provider'))
    ->withEntry('total_tokens', ref('input_tokens')->plus(ref('output_tokens')))
    ->withEntry('cost_usd', ref('input_tokens')->multiply(ref('price_usd_per_input_token'))
        ->plus(ref('output_tokens')->multiply(ref('price_usd_per_output_token'))))
    ->count());
measure('q4   the whole query, select + to_csv  [q4]', static function () use ($joined): int {
    $target = \sys_get_temp_dir() . '/profile-q4.csv';
    @\unlink($target);
    $joined()
        ->withEntry('day', ref('start')->stringBefore('T'))
        ->withEntry('provider', ref('price_provider'))
        ->withEntry('total_tokens', ref('input_tokens')->plus(ref('output_tokens')))
        ->withEntry('cost_usd', ref('input_tokens')->multiply(ref('price_usd_per_input_token'))
            ->plus(ref('output_tokens')->multiply(ref('price_usd_per_output_token'))))
        ->select('span_id', 'run_id', 'day', 'agent_id', 'model', 'provider', 'total_tokens', 'cost_usd', 'duration_ms')
        ->write(to_csv($target))
        ->run();
    return (int) \shell_exec('wc -l < ' . \escapeshellarg($target)) - 1;
});

// ── q4 written differently: the same rows, fewer objects per row ──────────────
//
// Each rewrite writes its output beside the baseline's, so the harness can show
// it is the same answer. The last two use `call()`, which the Query tab refuses
// — it takes a callable — so what they measure is what a named ScalarFunction
// in the service would buy, not something a query can write today.

/**
 * The 8-row price table as three lookups a row carries, instead of a join.
 *
 * @return array{provider: Flow\ETL\Function\ScalarFunction, input: Flow\ETL\Function\ScalarFunction, output: Flow\ETL\Function\ScalarFunction}
 */
function price_lookups(string $prices): array
{
    $handle = \fopen($prices, 'r');
    \fgetcsv($handle, escape: '');
    $provider = $input = $output = [];
    while (($row = \fgetcsv($handle, escape: '')) !== false) {
        $is = ref('model')->same(lit($row[0]));
        $provider[] = \Flow\ETL\DSL\match_condition($is, lit($row[1]));
        $input[] = \Flow\ETL\DSL\match_condition($is, lit((float) $row[2]));
        $output[] = \Flow\ETL\DSL\match_condition($is, lit((float) $row[3]));
    }
    \fclose($handle);

    return [
        'provider' => \Flow\ETL\DSL\match_cases($provider),
        'input' => \Flow\ETL\DSL\match_cases($input),
        'output' => \Flow\ETL\DSL\match_cases($output),
    ];
}

function rewritten(DataFrame $frame, string $prices, bool $native): DataFrame
{
    $lookup = price_lookups($prices);
    $frame = $frame
        ->filter(ref('status')->same(lit('ok'))->and(ref('kind')->same(lit('llm'))))
        ->withEntry('provider', $lookup['provider'])
        ->withEntry('price_in', $lookup['input'])
        ->withEntry('price_out', $lookup['output']);

    if (!$native) {
        return $frame
            ->withEntry('day', ref('start')->stringBefore('T'))
            ->withEntry('total_tokens', ref('input_tokens')->plus(ref('output_tokens')))
            ->withEntry('cost_usd', ref('input_tokens')->multiply(ref('price_in'))
                ->plus(ref('output_tokens')->multiply(ref('price_out'))));
    }

    return $frame
        ->withEntry('day', \Flow\ETL\DSL\call(
            static fn(string $start): string => \substr($start, 0, 10),
            [ref('start')],
            \Flow\Types\DSL\type_string(),
        ))
        ->withEntry('total_tokens', \Flow\ETL\DSL\call(
            static fn(int $in, int $out): int => $in + $out,
            [ref('input_tokens'), ref('output_tokens')],
            \Flow\Types\DSL\type_integer(),
        ))
        ->withEntry('cost_usd', \Flow\ETL\DSL\call(
            static fn(int $in, int $out, float $pin, float $pout): float => $in * $pin + $out * $pout,
            [ref('input_tokens'), ref('output_tokens'), ref('price_in'), ref('price_out')],
            \Flow\Types\DSL\type_float(),
        ));
}

function write_q4(DataFrame $frame, string $name): int
{
    $target = \sys_get_temp_dir() . "/{$name}.csv";
    @\unlink($target);
    $frame
        ->select('span_id', 'run_id', 'day', 'agent_id', 'model', 'provider', 'total_tokens', 'cost_usd', 'duration_ms')
        ->write(to_csv($target))
        ->run();

    return (int) \shell_exec('wc -l < ' . \escapeshellarg($target)) - 1;
}

$models = "{$corpus}/models.csv";
$q4columns = ['span_id', 'run_id', 'start', 'agent_id', 'model', 'status', 'kind', 'input_tokens', 'output_tokens', 'duration_ms'];

measure('q4x  join → 3 match_cases lookups, BigDecimal sums', static fn(): int
    => write_q4(rewritten($typed(), $models, native: false), 'profile-q4-lookup'));
measure('q4x  lookups + plain PHP arithmetic via call()', static fn(): int
    => write_q4(rewritten($typed(), $models, native: true), 'profile-q4-native'));
measure('q4x  lookups + call(), over Parquet with 10 columns projected', static fn(): int
    => write_q4(rewritten(
        data_frame()->read(from_parquet("{$corpus}/spans.parquet/part-*.parquet", columns: $q4columns)),
        $models,
        native: true,
    ), 'profile-q4-parquet'));
measure('q2x  q2 over Parquet with 6 columns projected', static fn(): int => \count(
    data_frame()
        ->read(from_parquet("{$corpus}/spans.parquet/part-*.parquet", columns: ['kind', 'model', 'span_id', 'input_tokens', 'output_tokens', 'duration_ms']))
        ->filter(ref('kind')->same(lit('llm')))
        ->groupBy(ref('model'))
        ->aggregate(count(ref('span_id')), sum(ref('input_tokens')), sum(ref('output_tokens')), average(ref('duration_ms')))
        ->fetch()
        ->toArray(),
));
