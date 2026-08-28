<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Enrichment;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * Making a query readable by a PHP parser, without changing what it says.
 *
 * The whole reason this exists: `->read(default)` is the canonical first line
 * of a query and is not valid PHP, so every PHP parser stops there. Substituting
 * the dataset's real name gets past that. The risk is substituting too much, so
 * most of these tests are about what enrichment leaves alone.
 */
final class EnrichmentTest extends TestCase
{
    private function apply(string $query): Enrichment
    {
        return Enrichment::apply($query, new Catalog(FakeApi::withDemoRuns(), 'http://api.test'));
    }

    public function test_a_bareword_dataset_becomes_its_real_name_quoted(): void
    {
        $enriched = $this->apply("data_frame()->read(default)->groupBy(ref('agent'))");

        // `default` is a PHP keyword; without this substitution the text does
        // not parse at all.
        self::assertStringContainsString("->read('runs')", $enriched->php);
        self::assertStringNotContainsString('read(default)', $enriched->php);
    }

    public function test_the_enriched_text_is_a_complete_php_file(): void
    {
        $enriched = $this->apply("data_frame()->read(spans)->groupBy(ref('tool'))");

        self::assertStringStartsWith('<?php', $enriched->php);
        // A query may end without a semicolon; a PHP file may not, and the
        // missing one would otherwise be reported as the only problem.
        self::assertStringEndsWith(';', \rtrim($enriched->php));

        // The real check: PHP's own lexer, in parse mode, now accepts it.
        \token_get_all($enriched->php, \TOKEN_PARSE);
        self::assertTrue(true, 'the enriched text parses as PHP');
    }

    public function test_a_dataset_name_inside_a_string_is_left_alone(): void
    {
        $enriched = $this->apply("data_frame()->read(default)->withEntry('runs', ref('run_id'))");

        // The literal 'runs' is a column name someone chose, not a dataset.
        self::assertStringContainsString("withEntry('runs', ref('run_id'))", $enriched->php);
    }

    public function test_a_function_call_is_not_mistaken_for_a_dataset(): void
    {
        // `ref` is not a dataset, and even if a dataset were ever named `ref`,
        // the trailing bracket says this is a call.
        $enriched = $this->apply("data_frame()->read(default)->groupBy(ref('agent'))");

        self::assertStringContainsString("ref('agent')", $enriched->php);
    }

    public function test_an_unknown_dataset_is_left_for_the_parser_to_explain(): void
    {
        $enriched = $this->apply('data_frame()->read(nonsense)');

        // Rewriting it would hide the one error message that lists the real
        // datasets.
        self::assertStringContainsString('read(nonsense)', $enriched->php);
    }

    public function test_an_offset_in_the_enriched_text_maps_back_to_what_was_typed(): void
    {
        $query = "data_frame()->read(default)->groupBy(ref('agent'))";
        $enriched = $this->apply($query);

        // `default` (7 chars) became `'runs'` (6), so everything after it sits
        // one character earlier — and the preamble shifts everything again. A
        // diagnostic that ignored both would point at the wrong character.
        $inEnriched = \strpos($enriched->php, "ref('agent')");
        self::assertIsInt($inEnriched);

        self::assertSame(\strpos($query, "ref('agent')"), $enriched->originalOffset($inEnriched));
    }

    public function test_an_offset_never_points_past_what_was_typed(): void
    {
        $query = 'data_frame()->read(default)';
        $enriched = $this->apply($query);

        // The appended semicolon is not in the original; a diagnostic about it
        // has to land on the last real character.
        self::assertLessThan(\strlen($query), $enriched->originalOffset(\strlen($enriched->php)));
    }

    public function test_an_offset_inside_the_preamble_lands_on_the_first_character(): void
    {
        $enriched = $this->apply('data_frame()->read(default)');

        self::assertSame(0, $enriched->originalOffset(0));
    }
}
