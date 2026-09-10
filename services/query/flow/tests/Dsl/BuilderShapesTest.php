<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * The language features the panel's graphical query builder is written against.
 *
 * `apps/panel/src/lib/query-builder.ts` turns clicked attributes into text and
 * this service decides whether that text runs. Its own tests say the compiler
 * emits what was meant; these say the emitted shapes still parse — which is a
 * different question with a different failure. Nothing here is generated from
 * the panel and it deliberately is not a copy of its output: what is pinned is
 * the handful of *capabilities* the builder leans on, each of which could
 * disappear under a Flow upgrade with no other test noticing.
 *
 * - `array_expand` before a `groupBy`, which is how a run's `agents` list
 *   becomes one row per agent
 * - `->or(...)` chained off `same`, which is what several values on one
 *   attribute compile to
 * - `average`, `median` and `max` as aggregations beside `count`
 * - `limit` *after* an `aggregate`, which is where the builder puts it
 * - `select` on a grain with no grouping at all
 *
 * A failure here is a builder that has started producing refusals, and the
 * message names which shape stopped working.
 */
final class BuilderShapesTest extends TestCase
{
    /** @return iterable<string, array{string}> */
    public static function shapes(): iterable
    {
        yield 'several values on one attribute are an or-chain' => ["data_frame()
            ->read(runs)
            ->withEntry('agent', array_expand(ref('agents')))
            ->filter(ref('agent')->same(lit('planner'))->or(ref('agent')->same(lit('estimator'))))
            ->groupBy(ref('agent'), ref('status'))
            ->aggregate(
                count(ref('run_id')->as('runs'))
            )
            ->sortBy(ref('runs')->desc())
            ->write(to_output(truncate: false))
            ->run();"];

        yield 'two attributes are two filters' => ["data_frame()
            ->read(runs)
            ->withEntry('runtime', array_expand(ref('runtimes')))
            ->filter(ref('runtime')->same(lit('planner-web')))
            ->filter(ref('workflow')->same(lit('house-import')))
            ->groupBy(ref('runtime'))
            ->aggregate(
                count(ref('run_id')->as('runs'))
            )
            ->sortBy(ref('runs')->desc())
            ->write(to_output(truncate: false))
            ->run();"];

        yield 'the statistics a grouped span query reports' => ["data_frame()
            ->read(spans)
            ->filter(ref('model')->same(lit('claude-opus-5')))
            ->groupBy(ref('model'), ref('agent_id'))
            ->aggregate(
                count(ref('span_id')->as('spans')),
                average(ref('duration_ms')->as('avg_duration_ms')),
                median(ref('duration_ms')->as('median_duration_ms')),
                max(ref('duration_ms')->as('max_duration_ms'))
            )
            ->sortBy(ref('spans')->desc())
            ->limit(100)
            ->write(to_output(truncate: false))
            ->run();"];

        yield 'nothing grouped selects columns instead' => ["data_frame()
            ->read(runs)
            ->filter(ref('status')->same(lit('failed')))
            ->select(ref('run_id'), ref('status'), ref('workflow'), ref('started_at'), ref('duration_ms'), ref('llm_calls'), ref('input_tokens'))
            ->sortBy(ref('started_at')->desc())
            ->limit(50)
            ->write(to_output(truncate: false))
            ->run();"];
    }

    #[DataProvider('shapes')]
    public function test_a_shape_the_builder_emits_still_parses(string $query): void
    {
        $catalog = new Catalog(FakeApi::withDemoRuns(), 'http://api.test');

        // Building validates every column against the catalog and reaches no
        // network; a refusal arrives as a ParseError, which is the failure.
        $plan = (new PipelineBuilder($catalog))->build(Parser::parse($query));

        self::assertNotNull($plan->frame);
    }
}
