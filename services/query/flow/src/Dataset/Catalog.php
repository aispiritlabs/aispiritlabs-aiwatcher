<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

use Flow\ETL\DataFrame;
use Nyholm\Psr7\Request;
use Psr\Http\Client\ClientInterface;

use function Flow\ETL\Adapter\Http\from_http_paginated;
use function Flow\ETL\Adapter\Http\http_pagination_cursor;
use function Flow\ETL\Adapter\Http\http_pagination_offset;
use function Flow\ETL\Adapter\Http\http_request_option_query;
use function Flow\ETL\Adapter\Http\http_stop_when_empty_path;
use function Flow\ETL\Adapter\Http\http_stop_when_max_results;
use function Flow\ETL\DSL\array_expand;
use function Flow\ETL\DSL\array_get;
use function Flow\ETL\DSL\cast;
use function Flow\ETL\DSL\data_frame;
use function Flow\ETL\DSL\optional;
use function Flow\ETL\DSL\ref;
use function Flow\Types\DSL\type_array;

/**
 * Every dataset a query can read, and how to turn one into a `DataFrame`.
 *
 * ## Why HTTP and not a local copy
 *
 * Measured on 1500 runs: `groupBy(agent)` over `/api/v1/runs` through Flow's
 * paginated HTTP extractor takes 210 ms. The same question over 175 000 raw
 * events in Parquet takes 2 s, and over the raw write-ahead log 16 s. What
 * decides it is the grain, not the transport — the API already folds events
 * into run summaries, so there are 1500 rows to read instead of 175 000.
 *
 * So there is no export, no ingest job and no second copy of the data. The
 * cursors the API already serves (`next_cursor` in the body, replayed as a
 * query parameter) are exactly the shape `http_pagination_cursor` expects.
 *
 * The cost is that a query only sees what the API serves, which is the read
 * model's retention window. History older than that needs the columnar path;
 * see ADR_0008 for the condition under which that gets built.
 *
 * ## What a dataset is, and where that is written
 *
 * Not here. `services/query/contract/catalog.json` declares every dataset — its
 * route, its columns, its hints, its `read()` arguments — and every query engine
 * loads that one file (AW-3), so the DataFusion and DuckDB engines serve exactly
 * the catalog this one does. What is Flow's is how a declared dataset becomes a
 * `DataFrame`, which is `open()` below.
 */
final readonly class Catalog
{
    /** @var array<string, Dataset> */
    private array $datasets;

    public function __construct(
        private ClientInterface $client,
        private string $baseUrl,
        /** How many rows to ask for per request. Fewer, larger pages beat many small ones. */
        private int $pageSize = 500,
        /**
         * A directory of part files, for the one dataset not served by the API.
         *
         * Null — the default — and the catalog is the API and nothing else. See
         * `Corpus` for why the exception exists and what it may read.
         */
        private ?string $corpusRoot = null,
        /** The declared catalog. Null is the one every engine ships with. */
        ?string $catalogFile = null,
    ) {
        // A corpus dataset is offered only where a corpus root is configured:
        // without one it names files that are not there, and "no spans" would be
        // an answer about the data when the truth is that nobody pointed at any.
        $this->datasets = \array_filter(
            CatalogFile::read($catalogFile ?? CatalogFile::defaultPath()),
            static fn(Dataset $dataset): bool => $dataset->corpus === null || $corpusRoot !== null,
        );
    }

    /**
     * The query string one page of a dataset is read with.
     *
     * The window is the panel's time control, forwarded to the API rather than
     * applied to the rows here: filtering after the fact would page through
     * everything to throw most of it away, and the API already answers the
     * question. Datasets that do not take one are read whole — see
     * `Dataset::$windowed`.
     */
    private static function query(
        Dataset $dataset,
        int $pageSize,
        ?int $windowSeconds,
        array $arguments = [],
        ?int $asOf = null,
    ): string {
        $parameters = ['limit' => $pageSize];

        if ($dataset->windowed && $windowSeconds !== null && $windowSeconds > 0) {
            $parameters['window_seconds'] = $windowSeconds;
        }

        // The end of the window, when a managed plan pinned one. Absent for
        // every panel query, which is what keeps a link somebody pastes meaning
        // "the last hour" at the moment it is opened. Present, the window is a
        // closed span and a retry five minutes later reads the same rows.
        if ($dataset->windowed && $asOf !== null) {
            $parameters['as_of'] = $asOf;
        }

        // Only what the dataset declared. The API rejects unknown query
        // parameters rather than ignoring them, so forwarding whatever a query
        // happened to name would turn a typo into a 400 about the whole read.
        foreach ($dataset->parameters as $name => $_) {
            if (!isset($arguments[$name]) || $arguments[$name] === '') {
                continue;
            }

            $parameters[$name] = $arguments[$name];
        }

        return \http_build_query($parameters);
    }

    /** `default` is `runs`: the grain the explorer shows, and the cheap one. */
    public function resolve(string $name): ?Dataset
    {
        return $this->datasets[$name === 'default' ? 'runs' : $name] ?? null;
    }

    /** @return array<string, Dataset> */
    public function all(): array
    {
        return $this->datasets;
    }

    /**
     * Open a dataset as a `DataFrame` with its columns already flat.
     *
     * The HTTP extractor yields one row per *response*, carrying the raw body
     * and the request that produced it. Three steps turn that into rows: decode
     * the body, explode the array of records into one row each, then project
     * the declared columns. Whoever writes the query sees run columns, never the
     * HTTP envelope.
     */
    /**
     * @param array<string, string> $arguments the read()'s declared named arguments
     * @param ?int                  $asOf      the instant a managed plan pinned its window to end at
     */
    public function open(
        Dataset $dataset,
        ?string $run = null,
        ?int $windowSeconds = null,
        array $arguments = [],
        ?int $asOf = null,
        ?int $inputLimit = null,
    ): DataFrame {
        if ($dataset->corpus !== null && $this->corpusRoot !== null) {
            return (new Corpus($this->corpusRoot))->open($dataset, $arguments, $inputLimit);
        }

        $path = $dataset->requiresRun
            ? \str_replace('{run}', \rawurlencode((string) $run), $dataset->path)
            : $dataset->path;

        $request = new Request(
            'GET',
            $this->baseUrl . $path . '?' . self::query($dataset, $this->pageSize, $windowSeconds, $arguments, $asOf),
        );

        $pagination = http_pagination_cursor('next_cursor', http_request_option_query($dataset->cursorParam));
        $rowLimit = null;
        if ($dataset->name === 'hub_rows') {
            $rowLimit = \filter_var($arguments['limit'] ?? '100', \FILTER_VALIDATE_INT, ['options' => [
                'min_range' => 1,
                'max_range' => 1000,
            ]]);
            $offset = \filter_var($arguments['offset'] ?? '0', \FILTER_VALIDATE_INT, ['options' => ['min_range' => 0]]);
            if ($rowLimit === false || $offset === false) {
                throw new \InvalidArgumentException(
                    'hub_rows requires limit: 1–1000 and a nonnegative integer offset.',
                );
            }
            if ($inputLimit !== null) {
                $rowLimit = \min($rowLimit, $inputLimit);
            }
            $pagination = http_pagination_offset(
                http_request_option_query('offset'),
                http_request_option_query('limit'),
                \min(100, $rowLimit),
                start_offset: $offset,
                stop_when: http_stop_when_empty_path('rows')->or(http_stop_when_max_results($rowLimit)),
            );
        }

        $frame = data_frame()
            ->read(from_http_paginated($this->client, $request, $pagination))
            ->withEntry('__body', cast(ref('response_body'), type_array()))
            ->withEntry('__row', array_expand(array_get(ref('__body'), $dataset->rowsPath)));

        if ($rowLimit !== null) {
            // The final HTTP page may extend past the requested sample. Cap
            // before user transforms, so aggregations see exactly that sample.
            $frame = $frame->limit($rowLimit);
        }

        foreach (\array_keys($dataset->columns) as $column) {
            // `optional` because the API omits null fields rather than sending
            // them as null, and a bare `array_get` throws on a missing path. A
            // run that succeeded has no `error` key at all; without this, one
            // successful run fails the whole query.
            $frame = $frame->withEntry($column, optional(array_get(ref('__row'), self::source($dataset, $column))));
        }

        // Drop the scaffolding, so a `select()`-free query does not return the
        // whole HTTP exchange alongside the data.
        return $frame->drop(
            ref('__body'),
            ref('__row'),
            ref('response_body'),
            ref('response_headers'),
            ref('response_status_code'),
            ref('response_protocol_version'),
            ref('response_reason_phrase'),
            ref('request_body'),
            ref('request_uri'),
            ref('request_headers'),
            ref('request_protocol_version'),
            ref('request_method'),
        );
    }

    /**
     * Where a column lives inside one record.
     *
     * Runs and spans are already flat. A recorded event nests everything except
     * `event_type` and `data` under `metadata`, and the producing service one
     * level deeper — flattening it here is what lets a query say
     * `ref('service')` instead of `array_get(ref('metadata'), 'source.service')`.
     */
    private static function source(Dataset $dataset, string $column): string
    {
        if ($dataset->name === 'annotation_images') {
            // An image head is `{project, image: {...}, review, ...}`. Reading
            // the record's fields as columns is what lets a query say
            // `ref('group_id')` rather than `array_get(ref('image'), 'group_id')`.
            return match ($column) {
                'review' => $column,
                default => 'image.' . $column,
            };
        }

        if ($dataset->name !== 'events') {
            return $column;
        }

        return match ($column) {
            'event_type', 'data' => $column,
            'service' => 'metadata.source.service',
            'sdk' => 'metadata.source.sdk',
            default => 'metadata.' . $column,
        };
    }
}
