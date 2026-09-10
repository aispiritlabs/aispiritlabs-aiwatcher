<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Fake;

use Nyholm\Psr7\Response;
use Psr\Http\Client\ClientInterface;
use Psr\Http\Message\RequestInterface;
use Psr\Http\Message\ResponseInterface;

/**
 * A stand-in for the aiwatcher API.
 *
 * Deliberately not clever: canned pages keyed by path, with the same cursor
 * contract the real routes serve (`next_cursor` in the body, replayed as a
 * query parameter). It exists to let the builder's behaviour be tested without
 * a server, not to simulate the projector.
 *
 * It also records every request, so a test can assert that a query which must
 * not fan out did not fan out.
 */
final class FakeApi implements ClientInterface
{
    /** @var list<string> */
    public array $requested = [];

    /** @param array<string, list<array<string, mixed>>> $pages path => pages of body */
    public function __construct(
        private readonly array $pages,
    ) {}

    public function sendRequest(RequestInterface $request): ResponseInterface
    {
        $uri = $request->getUri();
        $this->requested[] = (string) $uri;

        $body = $this->pages[$uri->getPath()] ?? null;

        if ($body === null) {
            return new Response(404, ['content-type' => 'application/json'], '{"code":"not_found"}');
        }

        // Which page: the cursor is the index of the next one, so the fake
        // exercises the same resume path the real cursors do.
        \parse_str($uri->getQuery(), $query);
        $index = isset($query['after']) || isset($query['before']) ? (int) ($query['after'] ?? $query['before']) : 0;

        return new Response(
            200,
            ['content-type' => 'application/json'],
            \json_encode($body[$index] ?? ['runs' => [], 'spans' => [], 'events' => []], \JSON_THROW_ON_ERROR),
        );
    }

    /**
     * Three runs across two agents, split over two pages.
     *
     * Two pages on purpose: a fake that answers everything in one response
     * would let a broken cursor pass.
     */
    public static function withDemoRuns(): self
    {
        $run = static fn(string $id, array $agents): array => [
            'run_id' => $id,
            'conversation_id' => 'conv-1',
            'workflow' => 'research-summary',
            'trace_id' => \str_repeat('a', 32),
            'status' => 'succeeded',
            'agents' => $agents,
            'runtimes' => ['agent-service'],
            'started_at' => '2026-08-28T10:00:00Z',
            'ended_at' => '2026-08-28T10:00:02Z',
            'duration_ms' => 2000,
            'event_count' => 12,
            'llm_calls' => 2,
            'tool_calls' => 1,
            'input_tokens' => 800,
            'output_tokens' => 200,
            'cached_tokens' => 0,
        ];

        $span = static fn(string $id, string $tool): array => [
            'run_id' => 'run-1',
            'trace_id' => \str_repeat('a', 32),
            'span_id' => $id,
            'name' => 'execute_tool ' . $tool,
            'kind' => 'client',
            'start' => '2026-08-28T10:00:00Z',
            'end' => '2026-08-28T10:00:01Z',
            'duration_ms' => 1000,
            'operation' => 'execute_tool',
            'agent_id' => 'researcher',
            'tool' => $tool,
        ];

        return new self([
            '/api/v1/runs' => [
                [
                    'runs' => [$run('run-1', ['researcher']), $run('run-2', ['researcher', 'writer'])],
                    'next_cursor' => '1',
                ],
                ['runs' => [$run('run-3', ['researcher'])]],
            ],
            '/api/v1/spans' => [
                ['spans' => [$span('aaaaaaaaaaaaaaaa', 'web_search'), $span('bbbbbbbbbbbbbbbb', 'read_file')]],
            ],
            '/api/v1/runs/run-1/events' => [
                [
                    'events' => [[
                        'event_type' => 'run.started',
                        'data' => [],
                        'metadata' => [
                            'run_id' => 'run-1',
                            'trace_id' => \str_repeat('a', 32),
                            'span_id' => \str_repeat('b', 16),
                            'span_key' => 'run',
                            'stream_position' => 1,
                            'occurred_at' => '2026-08-28T10:00:00Z',
                            'source' => ['service' => 'agent-service', 'sdk' => 'python'],
                        ],
                    ]],
                ],
            ],
        ]);
    }
}
