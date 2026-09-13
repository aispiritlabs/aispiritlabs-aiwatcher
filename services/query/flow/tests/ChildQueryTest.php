<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests;

use Aiwatcher\Flow\ChildQuery;
use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\ExecutionMemory;
use Aiwatcher\Flow\Failure;
use Aiwatcher\Flow\QueryCancelled;
use Aiwatcher\Flow\QueryFailed;
use Aiwatcher\Flow\QueryRunner;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * A managed query runs in a process of its own, so a cancel ends it wherever it is — in the
 * middle of one long read as much as between two batches.
 */
final class ChildQueryTest extends TestCase
{
    private const string KEY = 'exec-1/read/1';

    private string $directory;

    protected function setUp(): void
    {
        $this->directory = \sys_get_temp_dir() . '/aiwatcher-flow-child-' . \bin2hex(\random_bytes(8));
        \mkdir($this->directory . '/corpus/spans.csv', 0o700, true);
    }

    protected function tearDown(): void
    {
        foreach (['/corpus/spans.csv/*', '/*'] as $pattern) {
            $files = \glob($this->directory . $pattern);

            foreach (\is_array($files) ? $files : [] as $file) {
                \is_dir($file) ? null : \unlink($file);
            }
        }

        foreach (['/corpus/spans.csv', '/corpus', ''] as $directory) {
            \rmdir($this->directory . $directory);
        }
    }

    public function test_a_query_asked_to_stop_is_killed_where_it_is_and_answers_cancelled(): void
    {
        // The child never reaches a batch boundary: it asks for its own cancel, as the cancel
        // route would from another request, and goes on working.
        $memory = new ExecutionMemory($this->directory);
        $memory->started(self::KEY);
        $heartbeat = $this->directory . '/heartbeat';
        $child = $this->child(['cancelled', $this->directory, self::KEY, $heartbeat], $memory, 30);

        $started = \microtime(true);

        try {
            $child->run(self::KEY, self::request());
            self::fail('a cancelled query does not answer');
        } catch (QueryCancelled $cancelled) {
            self::assertSame(409, Failure::of($cancelled)->status);
        }

        self::assertLessThan(5.0, \microtime(true) - $started, 'stopped by the mark, not by its limit');
        self::assertStopped($heartbeat);
    }

    public function test_a_query_past_its_limit_is_killed_on_the_wall_clock(): void
    {
        $heartbeat = $this->directory . '/heartbeat';
        $child = $this->child(['stuck', $heartbeat], new ExecutionMemory($this->directory), 1);

        $started = \microtime(true);

        try {
            $child->run(self::KEY, self::request());
            self::fail('a query past its limit does not answer');
        } catch (\RuntimeException $error) {
            self::assertStringContainsString('ran past its 1-second limit', $error->getMessage());
            self::assertSame(502, Failure::of($error)->status);
        }

        self::assertLessThan(5.0, \microtime(true) - $started);
        self::assertStopped($heartbeat);
    }

    public function test_an_answer_larger_than_a_pipe_holds_arrives_whole(): void
    {
        // Read while the child writes: a request that waited for the exit first would wait
        // for ever on a child blocked writing into a full pipe.
        $child = $this->child(['loud', '4000000'], new ExecutionMemory($this->directory), 30);

        $result = $child->run(self::KEY, self::request());

        self::assertIsArray($result['rows']);
        self::assertSame(4_000_000, \strlen((string) ($result['rows'][0]['text'] ?? '')));
    }

    public function test_a_child_that_dies_without_an_answer_says_what_it_said(): void
    {
        $child = $this->child(['dies'], new ExecutionMemory($this->directory), 30);

        $this->expectException(\RuntimeException::class);
        $this->expectExceptionMessage('ended with status 3 and no answer: PHP Fatal error: Allowed memory size');
        $child->run(self::KEY, self::request());
    }

    public function test_the_real_child_answers_a_managed_query_as_the_request_would_and_is_noted(): void
    {
        $this->writeCorpus();
        $memory = new ExecutionMemory($this->directory);
        $environment = [...\getenv(), 'AIWATCHER_CORPUS_DIR' => $this->directory . '/corpus'];
        $root = \dirname(__DIR__);
        $runner = new QueryRunner(
            new Catalog(FakeApi::withDemoRuns(), 'http://127.0.0.1:8080', corpusRoot: $this->directory . '/corpus'),
            'http://127.0.0.1:8080',
            $memory,
            child: ChildQuery::fromRoot($root, $memory, 30, $environment),
        );
        $query =
            "data_frame()->read(corpus_spans, format: 'csv')->groupBy(ref('model'))"
            . "->aggregate(count(ref('span_id')->as('spans')))->sortBy(ref('model')->asc())";

        $managed = $runner->run($query, null, QueryRunner::MAX_ROWS, self::KEY);
        $adHoc = $runner->run($query);

        self::assertSame([['model' => 'a', 'spans' => 2], ['model' => 'b', 'spans' => 1]], $managed['rows']);
        self::assertSame($adHoc['digest'], $managed['digest']);
        self::assertSame(
            ['state' => ExecutionMemory::DONE, 'digest' => $managed['digest'], 'rows' => 2],
            $memory->seen(self::KEY),
        );
    }

    public function test_a_query_the_real_child_refused_is_answered_with_the_status_decided_there(): void
    {
        $memory = new ExecutionMemory($this->directory);
        $runner = new QueryRunner(
            new Catalog(FakeApi::withDemoRuns(), 'http://127.0.0.1:8080'),
            'http://127.0.0.1:8080',
            $memory,
            child: ChildQuery::fromRoot(\dirname(__DIR__), $memory, 30),
        );

        try {
            $runner->run('data_frame()->read(nothing_by_this_name)', null, QueryRunner::MAX_ROWS, self::KEY);
            self::fail('a query naming no dataset does not answer');
        } catch (QueryFailed $failed) {
            self::assertSame(422, Failure::of($failed)->status);
            self::assertStringContainsString('nothing_by_this_name', $failed->getMessage());
        }

        self::assertSame(['state' => ExecutionMemory::ABSENT], $memory->seen(self::KEY), 'a failure leaves no note');
    }

    /** @param list<string> $arguments */
    private function child(array $arguments, ExecutionMemory $memory, int $timeoutSeconds): ChildQuery
    {
        return new ChildQuery([\PHP_BINARY, __DIR__ . '/Fake/child.php', ...$arguments], $memory, $timeoutSeconds);
    }

    /** @return array{pipeline: string, window_seconds: ?int, max_rows: int, window_span: ?array{int, int}} */
    private static function request(): array
    {
        return [
            'pipeline' => 'data_frame()->read(default)',
            'window_seconds' => null,
            'max_rows' => 10,
            'window_span' => null,
        ];
    }

    /** The child's heartbeat does not grow once the request has answered: it is gone, not orphaned. */
    private static function assertStopped(string $heartbeat): void
    {
        \clearstatcache(true, $heartbeat);
        $before = \is_file($heartbeat) ? \filesize($heartbeat) : 0;
        \usleep(200_000);
        \clearstatcache(true, $heartbeat);
        self::assertSame($before, \is_file($heartbeat) ? \filesize($heartbeat) : 0, 'the child still runs');
    }

    private function writeCorpus(): void
    {
        $handle = \fopen($this->directory . '/corpus/spans.csv/part-00000.csv', 'w');
        self::assertNotFalse($handle);
        $columns = [
            'run_id',
            'trace_id',
            'span_id',
            'parent_span_id',
            'name',
            'kind',
            'start',
            'end',
            'duration_ms',
            'operation',
            'agent_id',
            'model',
            'tool',
            'step_type',
            'status',
            'input_tokens',
            'output_tokens',
            'error',
        ];
        \fputcsv($handle, $columns, escape: '');

        foreach ([['s1', 'a'], ['s2', 'a'], ['s3', 'b']] as [$span, $model]) {
            \fputcsv(
                $handle,
                [
                    'r1',
                    't1',
                    $span,
                    null,
                    'call',
                    'llm',
                    '2026-09-13T10:00:00Z',
                    '2026-09-13T10:00:01Z',
                    10,
                    'chat',
                    'agent',
                    $model,
                    null,
                    null,
                    'ok',
                    1,
                    1,
                    null,
                ],
                escape: '',
            );
        }

        \fclose($handle);
    }
}
