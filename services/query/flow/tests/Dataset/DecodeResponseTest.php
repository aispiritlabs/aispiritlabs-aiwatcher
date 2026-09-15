<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\DecodeResponse;
use PHPUnit\Framework\TestCase;

use function Flow\ETL\DSL\array_expand;
use function Flow\ETL\DSL\array_get;
use function Flow\ETL\DSL\data_frame;
use function Flow\ETL\DSL\from_array;
use function Flow\ETL\DSL\optional;
use function Flow\ETL\DSL\ref;

final class DecodeResponseTest extends TestCase
{
    public function test_dynamic_json_keeps_native_values_across_pages_and_missing_catalog_fields(): void
    {
        $payloads = [
            ['code' => '001', 'score' => null, 'nested' => ['flag' => true, 'labels' => []]],
            ['code' => '002', 'score' => 3.5, 'nested' => ['flag' => false, 'labels' => ['a']]],
        ];
        $responses = [];
        foreach ($payloads as $index => $payload) {
            $responses[] = ['response_body' => \json_encode(
                [
                    'rows' => [['row_index' => $index, 'row' => $payload]],
                ],
                \JSON_THROW_ON_ERROR,
            )];
        }
        $frame = data_frame()
            ->read(from_array($responses)->withBatchSize(1))
            ->transform(new DecodeResponse('rows', [
                'row_index' => 'int',
                'row' => 'array',
                'omitted' => 'list<string>',
            ]))
            ->withEntry('record', array_expand(array_get(ref('__body'), 'rows')))
            ->withEntry('payload', array_get(ref('record'), 'row'))
            ->withEntry('score', optional(array_get(ref('payload'), 'score')))
            ->withEntry('omitted', optional(array_get(ref('record'), 'omitted')))
            ->select('payload', 'score', 'omitted');

        $actual = [];
        foreach ($frame->get() as $batch) {
            $actual = [...$actual, ...$batch->toArray()];
        }
        self::assertSame($payloads, \array_column($actual, 'payload'));
        self::assertSame([null, 3.5], \array_column($actual, 'score'));
        self::assertSame([null, null], \array_column($actual, 'omitted'));
    }
}
