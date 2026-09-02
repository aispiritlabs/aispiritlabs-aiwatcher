<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

use Flow\ETL\DataFrame;
use Nyholm\Psr7\Request;
use Psr\Http\Client\ClientInterface;

use function Flow\ETL\Adapter\Http\from_http_paginated;
use function Flow\ETL\Adapter\Http\http_pagination_cursor;
use function Flow\ETL\Adapter\Http\http_request_option_query;
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
    ) {
        $this->datasets = self::definitions();
    }

    /** @return array<string, Dataset> */
    public static function definitions(): array
    {
        $runs = new Dataset(
            name: 'runs',
            path: '/api/v1/runs',
            rowsPath: 'runs',
            cursorParam: 'before',
            grain: 'one row per run',
            description: 'Runs the projector still retains, newest first. The default: it is the grain the explorer tree shows, and the fast one.',
            columns: [
                'run_id' => 'string',
                'conversation_id' => 'string|null',
                'workflow' => 'string|null',
                'trace_id' => 'string',
                'status' => 'string',
                'agents' => 'list<string>',
                'runtimes' => 'list<string>',
                'started_at' => 'string',
                'ended_at' => 'string|null',
                'duration_ms' => 'int|null',
                'event_count' => 'int',
                'llm_calls' => 'int',
                'tool_calls' => 'int',
                'input_tokens' => 'int',
                'output_tokens' => 'int',
                'cached_tokens' => 'int',
                'error' => 'string|null',
            ],
            hints: [
                // The single most likely first query, and the one place this
                // dataset's shape surprises people.
                'agent' =>
                    'A run can involve several agents, so the column is "agents", a list. '
                        . 'For one row per run and agent, add '
                        . '->withEntry(\'agent\', array_expand(ref(\'agents\'))) before the groupBy. '
                        . 'For one row per span with a single agent, read the "spans" dataset instead.',
                'runtime' =>
                    'A run can be produced by several services, so the column is "runtimes", a list. '
                        . 'Add ->withEntry(\'runtime\', array_expand(ref(\'runtimes\'))) before the groupBy.',
                'model' => 'A run does not carry the models it used; its spans do. Read the "spans" dataset.',
                'tool' => 'A run does not carry the tools it called; its spans do. Read the "spans" dataset.',
            ],
            windowed: true,
        );

        $spans = new Dataset(
            name: 'spans',
            path: '/api/v1/spans',
            rowsPath: 'spans',
            cursorParam: 'after',
            grain: 'one row per completed span',
            description: 'Every retained span, flat. Each row has exactly one agent, model and tool, so this is the dataset to group by any of them.',
            columns: [
                'run_id' => 'string',
                'trace_id' => 'string',
                'span_id' => 'string',
                'parent_span_id' => 'string|null',
                'name' => 'string',
                'kind' => 'string',
                'start' => 'string',
                'end' => 'string',
                'duration_ms' => 'int',
                'operation' => 'string|null',
                'agent_id' => 'string|null',
                'model' => 'string|null',
                'tool' => 'string|null',
                'step_type' => 'string|null',
            ],
            hints: [
                'agent' => 'The column is "agent_id".',
                'duration' => 'The column is "duration_ms".',
            ],
            windowed: true,
        );

        $events = new Dataset(
            name: 'events',
            path: '/api/v1/runs/{run}/events',
            rowsPath: 'events',
            cursorParam: 'after',
            grain: 'one row per recorded event',
            description: 'The raw log for one run, from the durable log rather than the read model. Needs a run: read(events, run: \'run-1\').',
            columns: [
                'event_type' => 'string',
                'run_id' => 'string',
                'conversation_id' => 'string|null',
                'workflow_id' => 'string|null',
                'agent_id' => 'string|null',
                'trace_id' => 'string',
                'span_id' => 'string',
                'parent_span_id' => 'string|null',
                'span_key' => 'string',
                'sequence' => 'int|null',
                'stream_position' => 'int',
                'occurred_at' => 'string',
                'service' => 'string',
                'sdk' => 'string',
                'data' => 'array',
            ],
            requiresRun: true,
        );

        $hubDatasets = new Dataset(
            name: 'hub_datasets',
            path: '/api/v1/dataset-hubs/search',
            rowsPath: 'results',
            // The hub search is capped rather than paged: the answer to "four
            // hundred matches" is a narrower query, not a cursor. There is no
            // `next_cursor` in the body, so this parameter is never replayed.
            cursorParam: 'unused',
            grain: 'one row per dataset a hub says it has',
            description: 'Kaggle and Hugging Face, searched live. A discovery surface, never a licence: every row is usage "unclear" unless it matches a corpus whose licence a human read at the original.',
            columns: [
                'hub' => 'string',
                'id' => 'string',
                'title' => 'string',
                'owner' => 'string',
                'url' => 'string',
                // Named for what it is. A mirror's licence field is what
                // somebody typed when uploading a copy.
                'claimed_license' => 'string',
                'usage' => 'string',
                'curated_source' => 'string|null',
                'downloads' => 'int|null',
                'likes' => 'int|null',
                'updated_at' => 'string',
                'tags' => 'list<string>',
            ],
            hints: [
                'license' =>
                    'The column is "claimed_license", and the name is the point: it is the mirror\'s word, not a licence. '
                        . '"usage" is aiwatcher\'s verdict and is "unclear" unless "curated_source" names a corpus somebody read at the original.',
                'name' => 'The columns are "id" (owner/name, how the hub addresses it) and "title".',
                'files' => 'The search does not list files. Open the dataset at its "url".',
            ],
            parameters: [
                'search' => new Parameter(
                    name: 'search',
                    required: false,
                    description: 'Free text. Omitted asks each hub what it considers popular, which is a worse question than any real one.',
                ),
                'hub' => new Parameter(
                    name: 'hub',
                    required: false,
                    description: 'One hub instead of both.',
                    values: ['kaggle', 'huggingface'],
                ),
                'limit' => new Parameter(
                    name: 'limit',
                    required: false,
                    description: 'Rows per hub, capped at 50 by the API.',
                ),
            ],
        );

        $annotationImages = new Dataset(
            name: 'annotation_images',
            path: '/api/v1/annotation-images',
            rowsPath: 'images',
            cursorParam: 'offset',
            grain: 'one row per registered image',
            description: 'What an annotation project already holds. The dataset an import pipeline joins against so a second run of it does not re-register what the first one did.',
            columns: [
                'image_id' => 'string',
                'uri' => 'string',
                'width' => 'int',
                'height' => 'int',
                // The **subject**, not the picture. A mirrored or re-shot
                // copy shares this with its original, which is what keeps them
                // on one side of the split.
                'group_id' => 'string',
                'source' => 'string',
                'review' => 'string',
                'level' => 'string|null',
            ],
            hints: [
                'project' => 'The project is a read() argument, not a column: read(annotation_images, project: \'corpora/first\').',
                'rights' => 'Not projected here; rights is a tagged object rather than a column. Read one image for it.',
                'family' => 'The column is "group_id" — the subject, however many pictures of it there are.',
            ],
            parameters: [
                'project' => new Parameter(
                    name: 'project',
                    required: true,
                    description: 'Which annotation project to read. There is no route without one.',
                ),
                'review' => new Parameter(
                    name: 'review',
                    required: false,
                    description: 'Only images in this review state.',
                    values: ['draft', 'in_review', 'accepted', 'rejected'],
                ),
                'split' => new Parameter(
                    name: 'split',
                    required: false,
                    description: 'Only one side of the family split.',
                    values: ['train', 'validation', 'test'],
                ),
            ],
        );

        return [
            'runs' => $runs,
            'spans' => $spans,
            'events' => $events,
            'hub_datasets' => $hubDatasets,
            'annotation_images' => $annotationImages,
        ];
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
    private static function query(Dataset $dataset, int $pageSize, ?int $windowSeconds, array $arguments = []): string
    {
        $parameters = ['limit' => $pageSize];

        if ($dataset->windowed && $windowSeconds !== null && $windowSeconds > 0) {
            $parameters['window_seconds'] = $windowSeconds;
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
    /** @param array<string, string> $arguments the read()'s declared named arguments */
    public function open(
        Dataset $dataset,
        ?string $run = null,
        ?int $windowSeconds = null,
        array $arguments = [],
    ): DataFrame {
        $path = $dataset->requiresRun
            ? \str_replace('{run}', \rawurlencode((string) $run), $dataset->path)
            : $dataset->path;

        $request = new Request(
            'GET',
            $this->baseUrl . $path . '?' . self::query($dataset, $this->pageSize, $windowSeconds, $arguments),
        );

        $frame = data_frame()
            ->read(from_http_paginated(
                $this->client,
                $request,
                http_pagination_cursor('next_cursor', http_request_option_query($dataset->cursorParam)),
            ))
            ->withEntry('__body', cast(ref('response_body'), type_array()))
            ->withEntry('__row', array_expand(array_get(ref('__body'), $dataset->rowsPath)));

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
