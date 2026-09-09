<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\ExecutionMemory;
use Aiwatcher\Flow\QueryRunner;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * The lookup half: what this service remembers, and what it refuses to.
 */
final class ExecutionMemoryTest extends TestCase
{
    private string $directory;

    protected function setUp(): void
    {
        $this->directory = \sys_get_temp_dir() . '/aiwatcher-flow-test-' . \bin2hex(\random_bytes(8));
        \mkdir($this->directory, 0o700, true);
    }

    protected function tearDown(): void
    {
        $files = \glob($this->directory . '/*');

        foreach (\is_array($files) ? $files : [] as $file) {
            \unlink($file);
        }
        \rmdir($this->directory);
    }

    public function test_a_key_nobody_ran_is_absent_rather_than_an_error(): void
    {
        // The ordinary answer and the safe one: the reactor treats it as "run it again",
        // and a retry of a deterministic query over the same window writes the same digest.
        self::assertSame(['state' => ExecutionMemory::ABSENT], $this->memory()->seen('exec-1/read/1'));
    }

    public function test_a_finished_query_is_remembered_by_what_it_hashed_to_and_not_by_its_rows(): void
    {
        // ADR 0014 refused this service an S3 client and that refusal stands: the rows go
        // to the artifact the reactor uploads, and what stays here is a note.
        $memory = $this->memory();
        $memory->finished('exec-1/read/1', 'abc123', 42);

        $seen = $memory->seen('exec-1/read/1');
        self::assertSame(ExecutionMemory::DONE, $seen['state']);
        self::assertSame('abc123', $seen['digest']);
        self::assertSame(42, $seen['rows']);
        self::assertArrayNotHasKey('rows_data', $seen);
    }

    public function test_a_running_query_tells_a_reactor_to_wait_rather_than_to_repeat_it(): void
    {
        // The one thing only this service can answer. A timeout says the caller stopped
        // waiting; it says nothing about whether the query is still executing here.
        $memory = $this->memory();
        $memory->started('exec-1/read/1');

        self::assertSame(ExecutionMemory::RUNNING, $memory->seen('exec-1/read/1')['state']);
    }

    public function test_a_query_that_failed_leaves_absent_rather_than_a_running_note(): void
    {
        // A `running` note nothing will ever complete would make the reactor wait until
        // its lease expired, for a query that stopped.
        $memory = $this->memory();
        $memory->started('exec-1/read/1');
        $memory->failed('exec-1/read/1');

        self::assertSame(ExecutionMemory::ABSENT, $memory->seen('exec-1/read/1')['state']);
    }

    public function test_a_note_older_than_the_lookup_window_is_absent(): void
    {
        // The window is the reactor's: it asks once, immediately after a timeout it did
        // not expect. A note that outlived that would be answering for nobody.
        $memory = $this->memory();
        $memory->finished('exec-1/read/1', 'abc123', 1);

        $path = $this->directory . '/' . \hash('sha256', 'exec-1/read/1') . '.json';
        $note = \json_decode((string) \file_get_contents($path), true);
        self::assertIsArray($note);
        $note['at'] = \time() - ExecutionMemory::TTL_SECONDS - 1;
        \file_put_contents($path, \json_encode($note, \JSON_THROW_ON_ERROR));

        self::assertSame(ExecutionMemory::ABSENT, $memory->seen('exec-1/read/1')['state']);
    }

    public function test_two_attempts_at_one_step_are_two_keys(): void
    {
        // The attempt number is in the key, which is why a note can be short-lived: there
        // is never a *next* attempt at the same key.
        $memory = $this->memory();
        $memory->finished('exec-1/read/1', 'abc123', 1);

        self::assertSame(ExecutionMemory::ABSENT, $memory->seen('exec-1/read/2')['state']);
    }

    public function test_a_key_holding_a_traversal_cannot_leave_the_directory(): void
    {
        // The key's parts are a caller's strings — an execution id from a request and a
        // step id from a canvas — and a path built from one verbatim would be a traversal
        // in a service that has no authentication of its own.
        $memory = $this->memory();
        $memory->finished('../../etc/passwd', 'abc123', 1);

        $written = \glob($this->directory . '/*.json');
        self::assertIsArray($written);
        self::assertCount(1, $written);
        self::assertSame(\hash('sha256', '../../etc/passwd') . '.json', \basename($written[0]));
    }

    public function test_an_ad_hoc_query_is_not_remembered_at_all(): void
    {
        // Every query the panel sends. They are not keyed, not resumed and not
        // deduplicated — ADR 0008's shape, unchanged — so there is nothing to note.
        $runner = $this->runner();
        $runner->run('data_frame()->read(default)');

        self::assertSame([], \glob($this->directory . '/*.json'));
    }

    public function test_a_managed_query_is_remembered_by_the_key_it_was_sent_under(): void
    {
        $runner = $this->runner();
        $result = $runner->run('data_frame()->read(default)', null, QueryRunner::MAX_ROWS, 'exec-1/read/1');

        $seen = $runner->seen('exec-1/read/1');
        self::assertSame(ExecutionMemory::DONE, $seen['state']);
        // The same fingerprint the answer carried, so a reactor can tell the note beside
        // its stored artifact from a note about a different run of that key.
        self::assertSame($result['digest'], $seen['digest']);
        self::assertSame($result['row_count'], $seen['rows']);
    }

    public function test_two_runs_of_one_deterministic_query_agree_on_the_digest(): void
    {
        // Which is what makes `absent` safe: the reactor retries, and the retry produces
        // the same answer.
        $runner = $this->runner();
        $first = $runner->run('data_frame()->read(default)', null, QueryRunner::MAX_ROWS, 'exec-1/read/1');
        $second = $runner->run('data_frame()->read(default)', null, QueryRunner::MAX_ROWS, 'exec-1/read/2');

        self::assertSame($first['digest'], $second['digest']);
    }

    public function test_a_pinned_span_is_read_as_a_span_and_the_answer_says_so(): void
    {
        // What makes a managed Flow step cacheable at all: the plan pins bounds,
        // the API's windowed routes take `as_of`, and a retry five minutes later
        // reads the same rows. The reactor believes the answer rather than the
        // request, because only this service knows what its sources accept.
        $api = FakeApi::withDemoRuns();
        $runner = new QueryRunner(new Catalog($api, 'http://api.test'), 'http://api.test', $this->memory());
        $result = $runner->run(
            'data_frame()->read(default)',
            3600,
            QueryRunner::MAX_ROWS,
            'exec-1/read/1',
            [1_700_000_000, 1_700_003_600],
        );

        self::assertSame('span', $result['window_applied']);
        self::assertSame(3600, $result['window_seconds']);
        self::assertStringContainsString('as_of=1700003600', $api->requested[0]);
        self::assertStringContainsString('window_seconds=3600', $api->requested[0]);
    }

    public function test_a_query_that_pinned_no_span_sends_no_end_and_says_none(): void
    {
        // Every query the panel sends. A shared link has to mean "the last
        // hour" when it is opened, not the hour it was copied.
        $api = FakeApi::withDemoRuns();
        $runner = new QueryRunner(new Catalog($api, 'http://api.test'), 'http://api.test', $this->memory());
        $result = $runner->run('data_frame()->read(default)', 3600);

        self::assertSame('none', $result['window_applied']);
        self::assertStringNotContainsString('as_of=', $api->requested[0]);
    }

    public function test_a_span_over_a_dataset_with_no_window_is_not_claimed_as_one(): void
    {
        // `events` is scoped by its run and takes no window, so a span cannot
        // be applied to it — and saying `span` would be a claim nobody could
        // act on.
        $runner = $this->runner();
        $result = $runner->run(
            "data_frame()->read(events, run: 'run-1')",
            3600,
            QueryRunner::MAX_ROWS,
            'exec-1/read/1',
            [1_700_000_000, 1_700_003_600],
        );

        self::assertSame('none', $result['window_applied']);
    }

    private function memory(): ExecutionMemory
    {
        return new ExecutionMemory($this->directory);
    }

    private function runner(): QueryRunner
    {
        return new QueryRunner(
            new Catalog(FakeApi::withDemoRuns(), 'http://api.test'),
            'http://api.test',
            $this->memory(),
        );
    }
}
