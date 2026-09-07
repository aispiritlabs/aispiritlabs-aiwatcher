<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Registry;
use Aiwatcher\Flow\ExecutionMemory;
use Aiwatcher\Flow\QueryRunner;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * What a query may name, now that the answer is derived rather than listed.
 *
 * The rejection half lives in [`ParserRejectionTest`], which is where the
 * cases that must keep failing belong. This is the other half: the categories
 * that must keep *working*, and the two things the derivation has to report
 * rather than merely allow.
 */
final class RegistryTest extends TestCase
{
    private string $directory;

    protected function setUp(): void
    {
        $this->directory = \sys_get_temp_dir() . '/aiwatcher-flow-registry-' . \bin2hex(\random_bytes(8));
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

    public function test_a_function_nobody_enumerated_is_available_because_flow_offers_it(): void
    {
        // `regex_replace` was never in the hand-written list and needed no arm
        // added for it. That is the whole point of deriving the vocabulary: a
        // Flow release arrives as capability rather than as a backlog item.
        $result = $this->answer(
            "data_frame()->read(default)->withEntry('tidy', regex_replace('/-/', '_', ref('run_id')))"
            . "->select(ref('tidy'))->limit(1)",
        );

        self::assertSame('run_1', $result['rows'][0]['tidy']);
    }

    public function test_a_query_that_asks_the_same_question_twice_is_reported_as_deterministic(): void
    {
        self::assertTrue(
            $this->answer("data_frame()->read(default)->select(ref('run_id'))->limit(1)")['deterministic'],
        );
    }

    public function test_a_query_that_names_the_clock_is_reported_as_not_deterministic(): void
    {
        // `now()` is honest work in the Query tab and a bad cache entry in a
        // managed step. Reported rather than refused, because only this service
        // knows which functions a query resolved — and the reactor already has
        // the seam that reads it, `ActivityResult::cacheable`.
        $result = $this->answer("data_frame()->read(default)->withEntry('t', now())->select(ref('t'))->limit(1)");

        self::assertFalse($result['deterministic']);
    }

    public function test_a_call_with_the_wrong_arguments_is_answered_with_the_signature(): void
    {
        // Flow's own TypeError names a parameter position in a file nobody
        // reading a query has open.
        $this->expectException(ParseError::class);
        $this->expectExceptionMessageMatches(
            '/regex_replace\(\) does not take those arguments\. It is regex_replace\(/',
        );

        $this->answer("data_frame()->read(default)->withEntry('x', regex_replace(ref('run_id')))");
    }

    public function test_a_function_that_opens_a_source_or_a_sink_is_not_in_the_vocabulary(): void
    {
        // The category rule, asserted where it is decided rather than only
        // through a query. A query composes values; reading and writing are
        // the catalog's and write()'s, and neither takes a name from the text.
        self::assertFalse(Registry::has('from_parquet'), 'an extractor is not a value');
        self::assertFalse(Registry::has('to_csv'), 'a loader is not a value');
        self::assertFalse(Registry::has('call'), 'a function taking a callable is refused by its signature');
        self::assertFalse(Registry::has('to_callable'), 'likewise');
    }

    public function test_a_name_declined_for_being_wrong_stays_declined_when_flow_still_offers_it(): void
    {
        // `equals` exists in Flow and works badly here: every column in these
        // datasets is nullable and it matches null against anything. Deriving
        // the vocabulary must not quietly re-admit what was refused on purpose.
        self::assertFalse(Registry::has('equals'));
        self::assertFalse(Registry::has('notEquals'));
        self::assertNotNull(\Aiwatcher\Flow\Dsl\Whitelist::declined('equals'));
    }

    public function test_window_functions_are_reachable_at_all(): void
    {
        // Six functions that had no arm and therefore did not exist here.
        foreach (['window', 'rank', 'dense_rank', 'row_number', 'unbounded_preceding'] as $name) {
            self::assertTrue(Registry::has($name), $name . ' is part of the query language');
        }
    }

    /** @return array<string, mixed> */
    private function answer(string $query): array
    {
        $runner = new QueryRunner(
            new Catalog(FakeApi::withDemoRuns(), 'http://api.test'),
            'http://api.test',
            new ExecutionMemory($this->directory),
        );

        return $runner->run($query);
    }
}
