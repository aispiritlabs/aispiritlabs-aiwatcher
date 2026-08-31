<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * Building, against a fake aiwatcher API.
 *
 * The fake is a PSR-18 client returning canned pages, so these run with no
 * server, no network and no timing — which is what makes it reasonable to
 * assert on the actual rows rather than on the shape of the pipeline.
 */
final class PipelineBuilderTest extends TestCase
{
    private function build(string $query, ?FakeApi $api = null): array
    {
        $catalog = new Catalog($api ?? FakeApi::withDemoRuns(), 'http://api.test');
        $plan = (new PipelineBuilder($catalog))->build(Parser::parse($query));

        return $plan->frame->fetch(100)->toArray();
    }

    public function test_grouping_runs_by_agent_needs_the_list_expanded_first(): void
    {
        // A run can involve several agents, so `runs` carries `agents` as a
        // list. Expanding it gives one row per run and agent — the same
        // semantics the Rust agent dimension uses.
        $rows = $this->build("data_frame()
                ->read(default)
                ->withEntry('agent', array_expand(ref('agents')))
                ->groupBy(ref('agent'))
                ->aggregate(count(ref('run_id')->as('runs')))
                ->write(to_output(truncate: false))
                ->run();");

        $byAgent = [];

        foreach ($rows as $row) {
            $byAgent[$row['agent']] = $row['runs'];
        }

        // researcher is in all three runs, writer only in run-2 — and run-3
        // arrives on the second page, so this also proves the cursor resumed.
        self::assertSame(['researcher' => 3, 'writer' => 1], $byAgent);
    }

    public function test_the_spans_dataset_groups_by_a_single_agent_without_expanding(): void
    {
        $rows = $this->build(
            "data_frame()->read(spans)->groupBy(ref('agent_id'))->aggregate(count(ref('span_id')->as('spans')))",
        );

        self::assertCount(1, $rows);
        self::assertSame('researcher', $rows[0]['agent_id']);
        self::assertSame(2, $rows[0]['spans']);
    }

    public function test_a_run_column_that_is_a_list_is_named_with_the_fix_spelled_out(): void
    {
        try {
            $this->build("data_frame()->read(default)->groupBy(ref('agent'))");
            self::fail('expected a parse error');
        } catch (ParseError $error) {
            // The most likely first query anyone writes. Saying only "unknown
            // column" would leave them guessing at a plural.
            self::assertStringContainsString('"agents", a list', $error->getMessage());
            self::assertStringContainsString('array_expand', $error->getMessage());
        }
    }

    public function test_a_misspelt_column_suggests_the_real_one(): void
    {
        try {
            $this->build("data_frame()->read(default)->groupBy(ref('run_di'))");
            self::fail('expected a parse error');
        } catch (ParseError $error) {
            self::assertStringContainsString('Did you mean "run_id"', $error->getMessage());
        }
    }

    public function test_a_column_added_by_with_entry_becomes_referenceable(): void
    {
        $rows = $this->build(
            "data_frame()->read(default)->withEntry('agent', array_expand(ref('agents')))->select(ref('agent'))",
        );

        self::assertNotEmpty($rows);
        self::assertArrayHasKey('agent', $rows[0]);
    }

    public function test_the_events_dataset_without_a_run_is_refused_rather_than_walking_every_run(): void
    {
        try {
            $this->build('data_frame()->read(events)');
            self::fail('expected a parse error');
        } catch (ParseError $error) {
            self::assertStringContainsString("read(events, run: 'run-1')", $error->getMessage());
            self::assertStringContainsString('a request per run', $error->getMessage());
        }
    }

    public function test_an_unknown_dataset_lists_the_real_ones(): void
    {
        try {
            $this->build('data_frame()->read(nonsense)');
            self::fail('expected a parse error');
        } catch (ParseError $error) {
            self::assertStringContainsString('runs, spans, events', $error->getMessage());
        }
    }

    public function test_a_sort_can_name_an_aggregation_output(): void
    {
        $rows = $this->build("data_frame()
                ->read(default)
                ->withEntry('agent', array_expand(ref('agents')))
                ->groupBy(ref('agent'))
                ->aggregate(count(ref('run_id')->as('runs')))
                ->sortBy(ref('runs')->desc())");

        self::assertSame('researcher', $rows[0]['agent']);
    }

    public function test_a_sink_outside_write_is_refused(): void
    {
        $this->expectExceptionMessage('to_output() belongs inside write()');

        $this->build("data_frame()->read(default)->withEntry('x', to_output())");
    }

    /**
     * The reason `equals` is not offered, kept as a live assertion.
     *
     * Measured against Flow 0.43: with a null in the column, `equals` matches
     * it and `notEquals` drops it — wrong in both directions. Since every
     * column in every dataset here is nullable, offering them by name would be
     * offering a filter that quietly returns the wrong rows. If a later Flow
     * fixes this, this test is where to notice.
     */
    public function test_the_loose_comparisons_are_declined_with_the_reason(): void
    {
        foreach (['equals' => '->same(', 'notEquals' => '->notSame('] as $method => $replacement) {
            try {
                $this->build(\sprintf(
                    "data_frame()->read(default)->filter(ref('status')->%s(lit('failed')))",
                    $method,
                ));
                self::fail('expected ' . $method . ' to be declined');
            } catch (ParseError $error) {
                self::assertStringContainsString('deliberately not available', $error->getMessage());
                // The message has to name the replacement, not just refuse.
                self::assertStringContainsString($replacement, $error->getMessage());
            }
        }
    }

    public function test_a_strict_filter_keeps_only_matching_rows(): void
    {
        $rows = $this->build(
            "data_frame()->read(spans)->filter(ref('tool')->same(lit('web_search')))->select(ref('tool'))",
        );

        self::assertCount(1, $rows);
        self::assertSame('web_search', $rows[0]['tool']);
    }

    public function test_any_combines_agent_filters_for_multi_agent_curation(): void
    {
        $rows = $this->build("data_frame()
            ->read(default)
            ->withEntry('agent', array_expand(ref('agents')))
            ->filter(any(
                ref('agent')->same(lit('writer')),
                ref('agent')->same(lit('reviewer'))
            ))
            ->dropDuplicates(ref('run_id'))");

        self::assertCount(1, $rows);
        self::assertSame('run-2', $rows[0]['run_id']);
    }

    public function test_curation_can_deduplicate_and_rename_columns(): void
    {
        $rows = $this->build("data_frame()
            ->read(default)
            ->dropDuplicates(ref('conversation_id'))
            ->rename('conversation_id', 'source_session_id')
            ->select(ref('source_session_id'))");

        self::assertCount(1, $rows);
        self::assertSame('conv-1', $rows[0]['source_session_id']);
    }

    public function test_truncate_false_reaches_the_plan(): void
    {
        $catalog = new Catalog(FakeApi::withDemoRuns(), 'http://api.test');

        $plain = (new PipelineBuilder($catalog))->build(Parser::parse('data_frame()->read(default)'));
        self::assertTrue($plain->truncate);

        $wide = (new PipelineBuilder($catalog))->build(Parser::parse(
            'data_frame()->read(default)->write(to_output(truncate: false))',
        ));
        self::assertFalse($wide->truncate);
    }
}
