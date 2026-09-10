<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\TitanicApi;
use PHPUnit\Framework\TestCase;

/**
 * The curation examples the panel ships, run against the corpus they name.
 *
 * These are the queries behind `Titanic · sex and status` and `Titanic ·
 * survival by status` in the Recipe view and the Flow half of
 * `curation/titanic-features` in the Pipeline view — see
 * `apps/panel/src/lib/flow.ts` and `apps/panel/src/lib/pipeline.ts`. They are
 * copied here rather than imported because one of them is TypeScript and the
 * other is PHP, and an example nobody runs is an example that stops working
 * quietly.
 *
 * The rows are ten passengers of the real training split, verbatim — 1, 2, 3,
 * 6, 8, 15, 31, 370, 444 and 642 — chosen because between them they are every
 * case worth having: the `Mlle.`, the `Mme.` and the `Ms.` a Kaggle notebook
 * folds in, the one `Don.` that belongs in no bucket, a passenger whose age
 * nobody recorded and one whose cabin nobody did. A fixture of clean rows
 * would pass while the shipped example failed on the corpus it names.
 */
final class TitanicCurationTest extends TestCase
{
    /** The title, as every one of these queries pulls it out of the name. */
    private const string TITLE = "->withEntry('title', regex_replace(lit('/^[^,]*,\\s*([^.]+)\\..*\$/'), lit('\$1'), ref('name')))";

    /** And the status, one `when` at a time, each step reading the last one's answer. */
    private const string STATUS = "
        ->withEntry('status', lit('rare'))
        ->withEntry('status', when(ref('title')->same(lit('Mr')), lit('mr'), ref('status')))
        ->withEntry('status', when(ref('title')->same(lit('Master')), lit('master'), ref('status')))
        ->withEntry('status', when(any(ref('title')->same(lit('Mrs')), ref('title')->same(lit('Mme'))), lit('mrs'), ref('status')))
        ->withEntry('status', when(any(ref('title')->same(lit('Miss')), ref('title')->same(lit('Mlle')), ref('title')->same(lit('Ms'))), lit('miss'), ref('status')))";

    private function rows(string $query): array
    {
        $catalog = new Catalog(new TitanicApi(), 'http://aiwatcher.test');
        $plan = (new PipelineBuilder($catalog))->build(Parser::parse($query));

        return $plan->frame->fetch(100)->toArray();
    }

    private function read(): string
    {
        return "data_frame()->read(hub_rows, dataset: 'phihung/titanic', split: 'train', limit: 100)
            ->withEntry('name', array_get(ref('row'), 'Name'))
            ->withEntry('sex', array_get(ref('row'), 'Sex'))
            ->withEntry('age', array_get(ref('row'), 'Age'))
            ->withEntry('cabin', array_get(ref('row'), 'Cabin'))
            ->withEntry('embarked', array_get(ref('row'), 'Embarked'))
            ->withEntry('pclass', array_get(ref('row'), 'Pclass'))
            ->withEntry('survived', array_get(ref('row'), 'Survived'))";
    }

    public function test_sex_becomes_a_number_without_deciding_what_a_missing_one_is(): void
    {
        $rows = $this->rows(
            $this->read()
            . "->withEntry('sex_code', when(ref('sex')->same(lit('female')), lit(1), lit(0)))"
            . "->select(ref('sex'), ref('sex_code'))->fetch()",
        );

        $seen = [];

        foreach ($rows as $row) {
            $seen[$row['sex']] = $row['sex_code'];
        }

        self::assertSame(['male' => 0, 'female' => 1], $seen);
    }

    public function test_the_title_in_the_name_becomes_a_status_with_the_variants_collapsed(): void
    {
        $rows = $this->rows(
            $this->read() . self::TITLE . self::STATUS . "->select(ref('title'), ref('status'))->fetch()",
        );

        $status = [];

        foreach ($rows as $row) {
            $status[$row['title']] = $row['status'];
        }

        self::assertSame(
            [
                'Mr' => 'mr',
                'Mrs' => 'mrs',
                'Miss' => 'miss',
                'Master' => 'master',
                // Everything the four buckets do not claim, by construction
                // rather than by a list: the first step writes 'rare' and
                // nothing later overwrote this row.
                'Don' => 'rare',
                // The three the Kaggle notebooks fold in, and the reason the
                // recode is a chain of `when` rather than four comparisons.
                'Mme' => 'mrs',
                'Ms' => 'miss',
                'Mlle' => 'miss',
            ],
            $status,
        );
    }

    public function test_a_missing_age_is_banded_as_unknown_rather_than_compared(): void
    {
        // `between()` throws on a null instead of answering false, so the null
        // branch has to be the outermost one. A band that compared first would
        // take the whole query down on the fifth of this corpus with no age.
        $rows = $this->rows(
            $this->read()
            . "->withEntry('age_band', when(ref('age')->isNull(), lit('unknown'),
                when(ref('age')->lessThan(lit(13)), lit('child'),
                when(ref('age')->lessThan(lit(20)), lit('teen'),
                when(ref('age')->lessThan(lit(60)), lit('adult'), lit('senior'))))))"
            . "->select(ref('age'), ref('age_band'))->fetch()",
        );

        $bands = [];

        foreach ($rows as $row) {
            $bands[] = $row['age_band'];
        }

        self::assertContains('child', $bands, 'the two-year-old');
        self::assertContains('teen', $bands, 'the fourteen-year-old');
        self::assertContains('adult', $bands);
        // Passenger 6, the one this corpus records no age for.
        self::assertSame(
            ['unknown'],
            \array_values(\array_unique(\array_filter($bands, static fn(string $band): bool => $band === 'unknown'))),
        );
        self::assertSame('unknown', $bands[3]);
    }

    public function test_the_deck_comes_off_the_cabin_and_a_missing_cabin_says_so(): void
    {
        $rows = $this->rows(
            $this->read()
            . "->withEntry('deck', when(ref('cabin')->isNull(), lit('unknown'), regex_replace(lit('/^(.).*\$/'), lit('\$1'), ref('cabin'))))"
            . "->select(ref('cabin'), ref('deck'))->fetch()",
        );

        self::assertSame('unknown', $rows[0]['deck'], 'no cabin recorded');
        self::assertSame('C', $rows[1]['deck'], 'C85');
    }

    public function test_survival_rate_by_status_and_class_is_one_grouped_query(): void
    {
        $rows = $this->rows($this->read() . self::TITLE . self::STATUS . "->groupBy(ref('status'), ref('sex'), ref('pclass'))
               ->aggregate(count(ref('survived')->as('passengers')), average(ref('survived')->as('survival_rate')))
               ->sortBy(ref('survival_rate')->desc())
               ->fetch()");

        $groups = [];

        foreach ($rows as $row) {
            $groups[$row['status'] . '/' . $row['pclass']] = [$row['passengers'], $row['survival_rate']];
        }

        // Two first-class `mrs`, and one of them is the `Mme.` — which is the
        // recode being measured rather than described.
        self::assertSame([2, 1.0], $groups['mrs/1']);
        // Two third-class `miss`, one of whom survived. `average` answers to
        // two decimals, so a query that rounded again would be rounding
        // somebody else's number.
        self::assertSame([2, 0.5], $groups['miss/3']);
        // Sorted by rate: the first row is not the worst-off group.
        self::assertGreaterThanOrEqual($rows[\count($rows) - 1]['survival_rate'], $rows[0]['survival_rate']);
    }
}
