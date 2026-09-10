<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Dsl\Plan;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

use function Flow\ETL\Adapter\Parquet\to_parquet;

/**
 * The one dataset read from disk rather than from the API.
 *
 * What the curation benchmark leans on: the per-model aggregation the
 * `curation/flow-vs-polars` example runs, over a corpus small enough to check
 * by hand, from both of the layouts the generator writes.
 */
final class CorpusTest extends TestCase
{
    /** The example pipeline's transform, verbatim, behind a read(). */
    private const string PER_MODEL =
        "->filter(ref('kind')->same(lit('llm')))"
            . "->groupBy(ref('model'))"
            . "->aggregate(count(ref('span_id')->as('spans')), sum(ref('input_tokens')->as('input_tokens')), "
            . "sum(ref('output_tokens')->as('output_tokens')), average(ref('duration_ms')->as('avg_duration_ms')))"
            . "->sortBy(ref('model')->asc())";

    /** Four model calls over two models, and two spans that are not model calls. */
    private const array EXPECTED = [
        ['model' => 'a', 'spans' => 2, 'input_tokens' => 30, 'output_tokens' => 6, 'avg_duration_ms' => 200.5],
        ['model' => 'b', 'spans' => 2, 'input_tokens' => 12, 'output_tokens' => 1, 'avg_duration_ms' => 60.5],
    ];

    private string $root;

    protected function setUp(): void
    {
        $this->root = \sys_get_temp_dir() . '/aiwatcher-corpus-' . \bin2hex(\random_bytes(6));
        \mkdir($this->root . '/spans.csv', recursive: true);

        // Two parts, so a glob that read only the first would be caught.
        $this->writePart('part-00000.csv', [
            self::span('s1', kind: 'llm', model: 'a', input: 10, output: 2, duration: 100),
            self::span('s2', kind: 'llm', model: 'a', input: 20, output: 4, duration: 301),
            self::span('s3', kind: 'tool', duration: 10),
        ]);
        $this->writePart('part-00001.csv', [
            self::span('s4', kind: 'llm', model: 'b', input: 5, output: 1, duration: 50),
            self::span(
                's5',
                kind: 'llm',
                model: 'b',
                input: 7,
                output: 0,
                duration: 71,
                error: 'upstream 503, retrying later',
            ),
            self::span('s6', kind: 'step', duration: 3),
        ]);
    }

    protected function tearDown(): void
    {
        $files = \glob($this->root . '/*/*');

        foreach ($files === false ? [] : $files as $file) {
            \unlink($file);
        }

        $directories = \glob($this->root . '/*');

        foreach ($directories === false ? [] : $directories as $directory) {
            \rmdir($directory);
        }

        \rmdir($this->root);
    }

    public function test_the_dataset_exists_only_where_a_corpus_root_is_configured(): void
    {
        self::assertNull((new Catalog(FakeApi::withDemoRuns(), 'http://api.test'))->resolve('corpus_spans'));
        self::assertNotNull($this->catalog()->resolve('corpus_spans'));

        $this->expectException(ParseError::class);
        $this->expectExceptionMessage('There is no dataset "corpus_spans"');
        (new PipelineBuilder(new Catalog(FakeApi::withDemoRuns(), 'http://api.test')))->build(Parser::parse(
            'data_frame()->read(corpus_spans)',
        ));
    }

    public function test_the_per_model_aggregation_reads_every_csv_part(): void
    {
        self::assertEquals(self::EXPECTED, $this->rows($this->plan('csv')));
    }

    public function test_the_parquet_copy_answers_the_same(): void
    {
        \mkdir($this->root . '/spans.parquet');
        $catalog = $this->catalog();
        $dataset = $catalog->resolve('corpus_spans');
        self::assertNotNull($dataset);
        $catalog
            ->open($dataset, arguments: ['format' => 'csv'])
            ->write(to_parquet($this->root . '/spans.parquet/part-00000.parquet'))
            ->run();

        self::assertEquals(self::EXPECTED, $this->rows($this->plan('parquet')));
    }

    public function test_a_read_from_disk_is_never_remembered(): void
    {
        // The files are addressed by nothing, so the same query tomorrow may
        // read different bytes; the reactor reads this as `cacheable`.
        self::assertFalse($this->plan('csv')->deterministic);
    }

    public function test_a_format_the_generator_does_not_write_is_refused_by_name(): void
    {
        $this->expectException(ParseError::class);
        $this->expectExceptionMessage("format: takes one of 'csv', 'parquet', not \"json\"");
        $this->plan('json');
    }

    public function test_a_missing_corpus_is_an_error_rather_than_an_empty_table(): void
    {
        $this->expectException(ParseError::class);
        $this->expectExceptionMessage('has no parquet part files');
        $this->plan('parquet');
    }

    public function test_a_preview_reads_a_sample_rather_than_the_corpus(): void
    {
        $plan = (new PipelineBuilder($this->catalog(), inputLimit: 2))->build(Parser::parse(
            'data_frame()->read(corpus_spans)',
        ));

        self::assertCount(2, $plan->frame->fetch(100)->toArray());
    }

    private function catalog(): Catalog
    {
        return new Catalog(FakeApi::withDemoRuns(), 'http://api.test', corpusRoot: $this->root);
    }

    private function plan(string $format): Plan
    {
        return (new PipelineBuilder($this->catalog()))->build(Parser::parse(
            "data_frame()->read(corpus_spans, format: '{$format}')" . self::PER_MODEL,
        ));
    }

    /** @return list<array<string, mixed>> */
    private function rows(Plan $plan): array
    {
        return $plan->frame->fetch(100)->toArray();
    }

    /** @param list<list<int|string|null>> $rows */
    private function writePart(string $name, array $rows): void
    {
        $handle = \fopen($this->root . '/spans.csv/' . $name, 'w');
        self::assertNotFalse($handle);
        \fputcsv(
            $handle,
            [
                'run_id',
                'trace_id',
                'span_id',
                'parent_span_id',
                'name',
                'kind',
                'start',
                'end',
                'duration_ms',
                'operation',
                'agent_id',
                'model',
                'tool',
                'step_type',
                'status',
                'input_tokens',
                'output_tokens',
                'error',
            ],
            escape: '',
        );

        foreach ($rows as $row) {
            \fputcsv($handle, $row, escape: '');
        }

        \fclose($handle);
    }

    /** @return list<int|string|null> */
    private static function span(
        string $id,
        string $kind,
        int $duration,
        ?string $model = null,
        ?int $input = null,
        ?int $output = null,
        ?string $error = null,
    ): array {
        return [
            'run-1',
            'trace-1',
            $id,
            $id === 's1' ? null : 's1',
            "{$kind}.x",
            $kind,
            '2026-08-01T00:00:00.000Z',
            '2026-08-01T00:00:01.000Z',
            $duration,
            null,
            'planner',
            $model,
            null,
            null,
            $error === null ? 'ok' : 'error',
            $input,
            $output,
            $error,
        ];
    }
}
