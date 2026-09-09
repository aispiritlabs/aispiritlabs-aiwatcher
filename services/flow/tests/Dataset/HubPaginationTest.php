<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\QueryRunner;
use Nyholm\Psr7\Response;
use PHPUnit\Framework\TestCase;
use Psr\Http\Client\ClientInterface;
use Psr\Http\Message\RequestInterface;
use Psr\Http\Message\ResponseInterface;

final class HubPaginationTest extends TestCase
{
    private function api(): ClientInterface
    {
        return new class implements ClientInterface {
            public array $requested = [];

            public function sendRequest(RequestInterface $request): ResponseInterface
            {
                \parse_str($request->getUri()->getQuery(), $query);
                $this->requested[] = $query;
                $offset = (int) ($query['offset'] ?? 0);
                $limit = \min(100, (int) ($query['limit'] ?? 100));
                $rows = [];
                for ($index = $offset; $index < \min(891, $offset + $limit); ++$index) {
                    $rows[] = ['row_index' => $index, 'row' => ['PassengerId' => $index + 1], 'omitted' => []];
                }
                return new Response(
                    200,
                    ['content-type' => 'application/json'],
                    \json_encode(['rows' => $rows, 'total_rows' => 891], \JSON_THROW_ON_ERROR),
                );
            }
        };
    }

    public function test_reads_all_passengers_across_hub_pages(): void
    {
        $api = $this->api();
        $runner = new QueryRunner(new Catalog($api, 'http://test'), 'test');
        $result = $runner->run("data_frame()->read(hub_rows, dataset: 'phihung/titanic', limit: 891)->fetch()");
        self::assertSame(891, $result['row_count']);
        self::assertSame(890, $result['rows'][890]['row_index']);
        self::assertCount(9, $api->requested);
        self::assertSame('800', $api->requested[8]['offset']);
        self::assertSame('100', $api->requested[0]['limit']);
    }

    public function test_sample_limit_and_offset_apply_before_aggregation(): void
    {
        $api = $this->api();
        $runner = new QueryRunner(new Catalog($api, 'http://test'), 'test');
        $result = $runner->run(
            "data_frame()->read(hub_rows, dataset: 'phihung/titanic', offset: 50, limit: 125)->aggregate(count(ref('row_index')->as('n')))->fetch()",
        );
        self::assertSame(125, $result['rows'][0]['n']);
        self::assertSame('50', $api->requested[0]['offset']);
        self::assertSame('150', $api->requested[1]['offset']);
        self::assertCount(2, $api->requested);
    }

    public function test_preview_fetches_only_one_page_and_keeps_its_output_cap(): void
    {
        $api = $this->api();
        $runner = new QueryRunner(new Catalog($api, 'http://test'), 'test');
        $result = $runner->run(
            "data_frame()->read(hub_rows, dataset: 'phihung/titanic', limit: 891)->fetch()",
            maxRows: 25,
        );
        self::assertSame(25, $result['row_count']);
        self::assertTrue($result['truncated']);
        self::assertCount(1, $api->requested);
    }

    public function test_stops_at_an_exhausted_split_and_refuses_invalid_limits(): void
    {
        $api = $this->api();
        $runner = new QueryRunner(new Catalog($api, 'http://test'), 'test');
        $result = $runner->run(
            "data_frame()->read(hub_rows, dataset: 'phihung/titanic', offset: 850, limit: 200)->fetch()",
        );
        self::assertSame(41, $result['row_count']);
        self::assertCount(2, $api->requested);
        $this->expectException(\InvalidArgumentException::class);
        $runner->run("data_frame()->read(hub_rows, dataset: 'phihung/titanic', limit: 1001)->fetch()");
    }
}
