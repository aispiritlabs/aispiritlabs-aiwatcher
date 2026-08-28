<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dsl;

use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Parser;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * The rejection half of the whitelist.
 *
 * These matter more than the acceptance tests. A whitelist is only worth
 * something if the things outside it are provably outside it, and the failure
 * mode being guarded against is not "the query does not work" — it is "the
 * query works, on the server, as the server's user".
 *
 * Each case below is a way someone might try to get PHP to run. None of them
 * may parse; a `ParseError` is the only acceptable outcome. If any of these
 * ever starts passing, the parser has stopped being a boundary.
 */
final class ParserRejectionTest extends TestCase
{
    /** @return iterable<string, array{string}> */
    public static function hostileQueries(): iterable
    {
        yield 'a bare function call' => ["system('id')"];
        yield 'a call smuggled into the chain' => ["data_frame()->read(default)->filter(system('id'))"];
        yield 'shell execution via backticks' => ['data_frame()->read(`id`)'];
        yield 'a closure argument' => ['data_frame()->read(default)->filter(fn($r) => 1)'];
        yield 'an anonymous function' => ['data_frame()->read(default)->filter(function ($r) { return 1; })'];
        yield 'a variable' => ['data_frame()->read($dataset)'];
        yield 'an include' => ["include '/etc/passwd';"];
        yield 'a require inside the chain' => ["data_frame()->read(default)->limit(require '/etc/passwd')"];
        yield 'eval' => ["eval('phpinfo();')"];
        yield 'object construction' => ['data_frame()->read(new Extractor())'];
        yield 'a static call' => ['data_frame()->read(Catalog::all())'];
        yield 'a namespaced call' => ["data_frame()->read(\\Flow\\ETL\\DSL\\from_array([]))"];
        yield 'closing the php tag' => ['data_frame() ?> <?php system("id");'];
        yield 'a second statement' => ["data_frame()->read(default)->run(); system('id');"];
        yield 'an assignment' => ['data_frame()->read(default); $x = 1;'];
        yield 'string interpolation' => ['data_frame()->read(default)->limit("${x}")'];
        yield 'echo' => ["echo 'x';"];
        yield 'a file read dressed as a scalar' => [
            "data_frame()->read(default)->withEntry('x', file_get_contents('/etc/passwd'))",
        ];
        yield 'exec through a whitelisted-looking name' => [
            "data_frame()->read(default)->withEntry('x', shell_exec('id'))",
        ];
        yield 'a method that is not a pipeline step' => ['data_frame()->getIterator()'];
        yield 'phpinfo' => ['data_frame()->read(default)->withEntry(\'x\', phpinfo())'];
    }

    #[DataProvider('hostileQueries')]
    public function test_a_hostile_query_is_a_parse_error_and_never_a_call(string $source): void
    {
        $this->expectException(ParseError::class);

        Parser::parse($source);
    }

    /**
     * The counterpart to the list above: nothing in it ran.
     *
     * The rejection tests prove the parser refuses. This proves the refusal
     * happens *before* anything executes — a parser that threw only after
     * calling `system()` would pass every test above.
     */
    public function test_nothing_executes_while_a_hostile_query_is_refused(): void
    {
        $marker = \sys_get_temp_dir() . '/aiwatcher-flow-parser-should-never-write-this';

        if (\is_file($marker)) {
            \unlink($marker);
        }

        foreach (self::hostileQueries() as [$source]) {
            try {
                Parser::parse($source);
            } catch (ParseError $expected) {
                self::assertNotSame('', $expected->getMessage(), 'a refusal explains itself');
            }
        }

        // A crude but decisive check: none of the refused queries reached a
        // function that could touch the filesystem.
        self::assertFileDoesNotExist($marker);
    }

    public function test_a_string_that_looks_like_a_call_stays_data(): void
    {
        // The quoting is the point: `system('id')` inside a string is a column
        // name, not syntax, and must survive as one.
        $query = Parser::parse("data_frame()->read(default)->withEntry('system(\\'id\\')', ref('run_id'))");

        $step = $query->stepNamed('withEntry');
        self::assertNotNull($step);
        self::assertSame("system('id')", $step->args[0]->value->value);
    }

    public function test_an_empty_query_is_refused_rather_than_returning_nothing(): void
    {
        $this->expectExceptionMessage('The query is empty.');

        Parser::parse('   ');
    }

    public function test_a_query_that_reads_nothing_says_so(): void
    {
        $this->expectExceptionMessage('The query reads nothing.');

        Parser::parse('data_frame()');
    }

    public function test_an_unknown_name_suggests_the_nearest_allowed_one(): void
    {
        try {
            Parser::parse("data_frame()->read(default)->groupby(ref('agent'))");
            self::fail('expected a parse error');
        } catch (ParseError $error) {
            self::assertStringContainsString('groupBy', $error->getMessage());
        }
    }

    public function test_an_alias_on_an_aggregation_names_the_correct_form(): void
    {
        try {
            Parser::parse("data_frame()->read(default)->aggregate(count(ref('run_id'))->as('runs'))");
            self::fail('expected a parse error');
        } catch (ParseError $error) {
            // Flow puts the alias on the reference. Saying only "not supported"
            // would leave someone stuck on the most common first mistake.
            self::assertStringContainsString("count(ref('…')->as(", $error->getMessage());
        }
    }

    public function test_an_error_points_at_the_offending_character(): void
    {
        try {
            Parser::parse("data_frame()->read(default)->nope(ref('agent'))");
            self::fail('expected a parse error');
        } catch (ParseError $error) {
            self::assertSame(\strpos("data_frame()->read(default)->nope(ref('agent'))", 'nope'), $error->column);
        }
    }
}
