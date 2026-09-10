<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\CatalogFile;
use PHPUnit\Framework\TestCase;

final class CatalogFileTest extends TestCase
{
    private string $file;

    protected function setUp(): void
    {
        $this->file = \sys_get_temp_dir() . '/catalog-' . \bin2hex(\random_bytes(6)) . '.json';
    }

    protected function tearDown(): void
    {
        if (\is_file($this->file)) {
            \unlink($this->file);
        }
    }

    public function test_the_catalog_every_engine_serves_is_the_one_flow_serves(): void
    {
        $datasets = CatalogFile::read(CatalogFile::defaultPath());

        self::assertSame(
            ['runs', 'spans', 'events', 'hub_datasets', 'hub_rows', 'hub_columns', 'annotation_images', 'corpus_spans'],
            \array_keys($datasets),
        );
        self::assertTrue($datasets['runs']->windowed);
        self::assertTrue($datasets['events']->requiresRun);
        self::assertSame('spans', $datasets['corpus_spans']->corpus);
        self::assertSame(['csv', 'parquet'], $datasets['corpus_spans']->parameters['format']->values);
    }

    public function test_a_near_miss_key_is_refused_by_name_rather_than_left_at_its_default(): void
    {
        // `windowed` defaulting to false is a query that sends its window nowhere and
        // reads everything, with nothing anywhere saying so.
        $this->write(['name' => 'runs', 'grain' => 'g', 'description' => 'd', 'columns' => [], 'window' => true]);

        $this->expectExceptionMessage('dataset "runs"');
        $this->expectExceptionMessage('"window"');

        CatalogFile::read($this->file);
    }

    public function test_a_comment_is_the_one_key_every_level_skips(): void
    {
        $this->write([
            '$comment' => 'why this dataset is shaped the way it is',
            'name' => 'hub_datasets',
            'grain' => 'g',
            'description' => 'd',
            'columns' => ['$comment' => 'the mirror\'s word, named for what it is', 'claimed_license' => 'string'],
            'parameters' => [['$comment' => 'q, not search', 'name' => 'q', 'required' => false, 'description' => 'd']],
        ]);

        $dataset = CatalogFile::read($this->file)['hub_datasets'];

        self::assertSame(['claimed_license' => 'string'], $dataset->columns);
        self::assertFalse($dataset->parameters['q']->required);
    }

    /** @param array<string, mixed> $dataset */
    private function write(array $dataset): void
    {
        \file_put_contents($this->file, \json_encode(['version' => 1, 'datasets' => [$dataset]], \JSON_THROW_ON_ERROR));
    }
}
