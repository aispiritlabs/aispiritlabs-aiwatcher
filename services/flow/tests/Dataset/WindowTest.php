<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * Relative periods are applied at the API read boundary.
 *
 * A script-level period wins over the panel fallback, making a saved recipe
 * reproducible. The aiwatcher API rejects unknown query parameters, so it must
 * still stay away from the per-run events route.
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

    public function test_a_script_period_overrides_the_panel_window(): void
    {
        $api = $this->read("data_frame()->read(default, period: '6h')", 900);

        foreach ($api->requested as $url) {
            self::assertStringContainsString('window_seconds=21600', $url);
            self::assertStringNotContainsString('window_seconds=900', $url);
        }
    }

    public function test_all_in_the_script_disables_the_panel_window(): void
    {
        $api = $this->read("data_frame()->read(default, period: 'all')", 900);

        foreach ($api->requested as $url) {
            self::assertStringNotContainsString('window_seconds', $url);
        }
    }

    public function test_a_numeric_period_is_interpreted_as_seconds(): void
    {
        $api = $this->read('data_frame()->read(default, period: 3600)', null);

        foreach ($api->requested as $url) {
            self::assertStringContainsString('window_seconds=3600', $url);
        }
    }

    public function test_an_invalid_period_explains_the_supported_formats(): void
    {
        $this->expectExceptionMessage("'15m', '6h', '7d', '2w', 'all', or seconds");

        $this->read("data_frame()->read(default, period: 'yesterday')", null);
    }

    public function test_a_per_run_dataset_refuses_a_period_it_cannot_apply(): void
    {
        $this->expectExceptionMessage('does not take a period');

        $this->read("data_frame()->read(events, run: 'run-1', period: '1h')", null);
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
