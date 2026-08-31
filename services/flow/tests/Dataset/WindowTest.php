<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * The panel's time window, forwarded to the API.
 *
 * The window is not part of the query language — a query says what to read,
 * and the period is a view control the whole panel shares. What has to hold is
 * that it reaches the routes that accept it and stays away from the one that
 * does not: the aiwatcher API rejects unknown query parameters, so sending
 * `window_seconds` to the per-run events route would turn a scoped query into
 * a 400.
 */
final class WindowTest extends TestCase
{
    private function read(string $query, ?int $windowSeconds): FakeApi
    {
        $api = FakeApi::withDemoRuns();
        $catalog = new Catalog($api, 'http://api.test');
        (new PipelineBuilder($catalog, $windowSeconds))->build(Parser::parse($query))->frame->fetch(100)->toArray();

        return $api;
    }

    public function test_a_windowed_dataset_asks_the_api_for_the_window(): void
    {
        $api = $this->read('data_frame()->read(default)', 900);

        self::assertNotSame([], $api->requested);

        foreach ($api->requested as $url) {
            self::assertStringContainsString('window_seconds=900', $url);
        }
    }

    public function test_no_window_reads_everything_rather_than_sending_a_zero(): void
    {
        $api = $this->read('data_frame()->read(default)', null);

        foreach ($api->requested as $url) {
            self::assertStringNotContainsString('window_seconds', $url);
        }
    }

    /**
     * A run's log is bounded by the run, and the route takes no window.
     *
     * `deny_unknown_fields` on the Rust side means this is not a harmless
     * extra parameter; it is the difference between a table and a 400.
     */
    public function test_the_per_run_events_route_never_receives_a_window(): void
    {
        $api = $this->read("data_frame()->read(events, run: 'run-1')", 900);

        self::assertNotSame([], $api->requested);

        foreach ($api->requested as $url) {
            self::assertStringNotContainsString('window_seconds', $url);
        }
    }
}
