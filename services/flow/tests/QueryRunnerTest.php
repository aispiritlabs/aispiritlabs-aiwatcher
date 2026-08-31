<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\QueryRunner;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

final class QueryRunnerTest extends TestCase
{
    public function test_a_simulation_cap_is_applied_while_rows_are_fetched(): void
    {
        $runner = new QueryRunner(new Catalog(FakeApi::withDemoRuns(), 'http://api.test'), 'http://api.test');
        $result = $runner->run('data_frame()->read(default)', null, 2);

        self::assertSame(2, $result['row_count']);
        self::assertTrue($result['truncated']);
        self::assertCount(2, $result['rows']);
    }

    public function test_a_requested_cap_can_never_widen_the_service_limit(): void
    {
        $runner = new QueryRunner(new Catalog(FakeApi::withDemoRuns(), 'http://api.test'), 'http://api.test');
        $result = $runner->run('data_frame()->read(default)', null, QueryRunner::MAX_ROWS + 10);

        self::assertSame(3, $result['row_count']);
        self::assertFalse($result['truncated']);
    }

    public function test_the_result_reports_the_period_embedded_in_the_script(): void
    {
        $runner = new QueryRunner(new Catalog(FakeApi::withDemoRuns(), 'http://api.test'), 'http://api.test');
        $result = $runner->run("data_frame()->read(default, period: '6h')", 900);

        self::assertSame(21_600, $result['window_seconds']);
    }

    public function test_all_in_the_script_is_reported_as_an_unbounded_period(): void
    {
        $runner = new QueryRunner(new Catalog(FakeApi::withDemoRuns(), 'http://api.test'), 'http://api.test');
        $result = $runner->run("data_frame()->read(default, period: 'all')", 900);

        self::assertNull($result['window_seconds']);
    }
}
