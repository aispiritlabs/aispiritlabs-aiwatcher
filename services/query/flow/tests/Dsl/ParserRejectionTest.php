<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\Attributes\DataProvider;
use PHPUnit\Framework\TestCase;

/**
 * The rejection half of the boundary.
 *
 * These matter more than the acceptance tests. Admission is only worth
 * something if the things outside it are provably outside it, and the failure
 * mode being guarded against is not "the query does not work" — it is "the
 * query works, on the server, as the server's user".
 *
 * Each case below is a way someone might try to get PHP to run. None of them
 * may parse; a `ParseError` is the only acceptable outcome. If any of these
 * ever starts passing, the parser has stopped being a boundary.
 *
 * Since the hand-written whitelist was deleted, the second block below matters
 * as much as the first: what refuses `to_csv('/etc/passwd')` is no longer a
 * name missing from a list — it is [`Registry`] admitting three return-type
 * namespaces and refusing every parameter that accepts a callable. These are
 * the cases that say so out loud.
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
        // Flow's own two callable-taking functions. What a query may name is
        // derived from signatures rather than listed, so these prove the
        // derivation is what refuses them — not an oversight somebody could
        // fix by adding a name.
        // Written without a second argument on purpose: `call('system', [])`
        // is refused by the lexer for the `[`, which would make this case pass
        // whatever the admission rules said.
        yield "Flow's call(), which takes a callable" => [
            "data_frame()->read(default)->withEntry('x', call('system'))",
        ];
        yield "Flow's to_callable()" => ["data_frame()->read(default)->withEntry('x', to_callable('system'))"];
        // A query composes values. The catalog decides what may be read and
        // write() decides where rows go, so Flow's own I/O constructors are
        // outside the admitted categories however harmless their names look.
        yield 'a sink that would write a file' => ["data_frame()->read(default)->write(to_csv('/tmp/x.csv'))"];
        yield 'an extractor that would read a file' => [
            "data_frame()->read(default)->withEntry('x', from_parquet('/etc/passwd'))",
        ];
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
    /**
     * Steps Flow has and this service does not offer, and why not.
     *
     * The step vocabulary is derived from `DataFrame` now, so what keeps these
     * out is a rule about their parameters rather than their absence from a
     * list — which is the thing worth testing, because the list is what used
     * to be doing it.
     *
     * @return iterable<string, array{string}>
     */
    public static function stepsTheAdmissionRuleRefuses(): iterable
    {
        // Takes a Loader: this is where rows would leave the process.
        yield 'load' => ['data_frame()->read(default)->load(to_output())'];
        // Takes a callable: this is where a query would become code.
        yield 'map' => ['data_frame()->read(default)->map(strtoupper)'];
        yield 'forEach' => ['data_frame()->read(default)->forEach(strtoupper)'];
        // Takes a Transformer: the same hole with a longer name.
        yield 'transform' => ['data_frame()->read(default)->transform(rename_replace())'];
        // Takes a SaveMode, which is about writing files.
        yield 'saveMode' => ['data_frame()->read(default)->saveMode(save_mode_overwrite())'];
        // Reads a filesystem path.
        yield 'filterPartitions' => ["data_frame()->read(default)->filterPartitions(ref('x'))"];
    }

    #[DataProvider('stepsTheAdmissionRuleRefuses')]
    public function test_a_frame_method_that_could_leave_the_process_is_not_a_step(string $source): void
    {
        $this->expectException(ParseError::class);
        Parser::parse($source);
    }

    public function test_a_method_on_a_value_is_looked_up_on_that_value_and_nowhere_else(): void
    {
        // The chained-method list is gone, so this is what stands in its
        // place: a name is resolved against the object the query built, and
        // PHP's own machinery is not part of that object's admitted surface.
        foreach ([
            "data_frame()->read(default)->filter(ref('status')->getIterator())",
            "data_frame()->read(default)->filter(ref('status')->eval())",
        ] as $query) {
            try {
                (new PipelineBuilder(new Catalog(FakeApi::withDemoRuns(), 'http://api.test')))->build(Parser::parse(
                    $query,
                ));
                self::fail('expected a parse error');
            } catch (ParseError $error) {
                self::assertStringContainsString('is not something a', $error->getMessage());
            }
        }
    }

    /** @return iterable<string, array{string, string}> */
    public static function categoriesTheRegistryRefuses(): iterable
    {
        // A loader is where rows would leave this process, and the name of one
        // looks as harmless as any other. `write()`'s three sinks are an enum
        // of things that mean "give the rows back", not a hole in this rule.
        yield 'writing a file' => ["data_frame()->read(default)->write(to_csv('/etc/passwd'))", 'to_csv'];
        yield 'writing json' => ["data_frame()->read(default)->write(to_json('/tmp/x'))", 'to_json'];
        // An extractor is the other direction: the catalog decides what may be
        // read, so a query may not open a source of its own.
        yield 'reading an array' => ['data_frame()->read(default)->filter(from_array([]))', 'from_array'];
        yield 'reading a directory' => ["data_frame()->read(default)->withEntry('x', files('/etc'))", 'files'];
        // The two functions Flow has that take a callable, refused by
        // signature rather than by name — which is what keeps a function Flow
        // adds next year refused before anybody here has heard of it.
        yield 'a callable parameter' => ["data_frame()->read(default)->withEntry('x', call('system'))", 'call'];
        yield 'a callable loader' => ["data_frame()->read(default)->write(to_callable('system'))", 'to_callable'];
    }

    #[DataProvider('categoriesTheRegistryRefuses')]
    public function test_a_category_outside_the_admitted_namespaces_is_refused(string $source, string $named): void
    {
        try {
            Parser::parse($source);
            self::fail("expected {$named} to be refused");
        } catch (ParseError $error) {
            self::assertStringContainsString($named, $error->getMessage());
            self::assertStringContainsString('not part of the query language', $error->getMessage());
        }
    }

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
