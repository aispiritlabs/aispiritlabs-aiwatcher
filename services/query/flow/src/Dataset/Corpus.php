<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

use Aiwatcher\Flow\Dsl\ParseError;
use Flow\ETL\DataFrame;
use Flow\ETL\Schema;

use function Flow\ETL\Adapter\CSV\from_csv;
use function Flow\ETL\Adapter\Parquet\from_parquet;
use function Flow\ETL\DSL\data_frame;
use function Flow\ETL\DSL\int_schema;
use function Flow\ETL\DSL\schema;
use function Flow\ETL\DSL\str_schema;

/**
 * A corpus on disk: part files under one root, read the way Flow reads files.
 *
 * Every other dataset is an aiwatcher API route, and `Catalog` says why — the
 * API has already folded events to the grain a question wants. This is the one
 * exception, and it exists to measure Flow itself over a corpus larger than any
 * read model holds (`benchmarks/curation`). It is off unless
 * `AIWATCHER_CORPUS_DIR` names a root, and a query names the dataset, never a
 * path under it.
 *
 * The files are typed on read from the column list: the contract the API
 * datasets keep, where what `/flow/datasets` shows is what a query can use.
 * CSV carries no types of its own to disagree with it.
 */
final readonly class Corpus
{
    /**
     * The dataset itself — `corpus_spans`, the synthetic spans
     * `benchmarks/curation/generate.py` writes — is declared in
     * `services/query/contract/catalog.json` with every other one; this reads it.
     */
    public function __construct(
        private string $root,
    ) {}

    /** @param array<string, string> $arguments the read()'s declared named arguments */
    public function open(Dataset $dataset, array $arguments, ?int $inputLimit): DataFrame
    {
        $format = ($arguments['format'] ?? '') === '' ? 'csv' : $arguments['format'];
        $directory = \sprintf('%s/%s.%s', \rtrim($this->root, '/'), (string) $dataset->corpus, $format);
        $pattern = "{$directory}/part-*.{$format}";

        // Refused rather than read as empty: a glob that matches nothing is a
        // table with no rows, and "no spans" is an answer about the data when
        // the truth is that nobody generated any.
        $parts = \glob($pattern);

        if ($parts === false || $parts === []) {
            throw new ParseError(\sprintf(
                '%s has no %s part files in %s. Generate them with `just bench-curation-generate`, '
                . 'or point AIWATCHER_CORPUS_DIR at a directory that holds them.',
                $dataset->name,
                $format,
                $directory,
            ));
        }

        $frame = data_frame()->read(match ($format) {
            'parquet' => from_parquet($pattern),
            default => from_csv($pattern, schema: self::schemaOf($dataset)),
        });

        // A preview reads a sample, never the corpus — the bound `hub_rows`
        // keeps, applied before any transform so an aggregation sees exactly it.
        return $inputLimit === null ? $frame : $frame->limit($inputLimit);
    }

    private static function schemaOf(Dataset $dataset): Schema
    {
        $definitions = [];

        foreach ($dataset->columns as $name => $type) {
            $nullable = \str_ends_with($type, '|null');
            $definitions[] = \str_starts_with($type, 'int')
                ? int_schema($name, $nullable)
                : str_schema($name, $nullable);
        }

        return schema(...$definitions);
    }
}
