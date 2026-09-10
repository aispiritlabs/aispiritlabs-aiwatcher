<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Statistics;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\TitanicApi;
use PHPUnit\Framework\TestCase;

/**
 * Asking a query for a distribution, over the corpus the panel's examples read.
 *
 * Flow ships "how many" and "how much" and nothing that says how a column is
 * spread, which is half of what anybody opens a curation to find out. These
 * four names are this service's own — see `src/Statistics` — and everything
 * around them is Flow's: they are aggregations, they run under `groupBy()`,
 * and a later `sortBy()` can name what they wrote.
 */
final class StatisticTest extends TestCase
{
    private function rows(string $query): array
    {
        $catalog = new Catalog(new TitanicApi(), 'http://aiwatcher.test');
        $plan = (new PipelineBuilder($catalog))->build(Parser::parse($query));

        return $plan->frame->fetch(100)->toArray();
    }

    private function read(): string
    {
        return "data_frame()->read(hub_rows, dataset: 'phihung/titanic')
            ->withEntry('name', array_get(ref('row'), 'Name'))
            ->withEntry('sex', array_get(ref('row'), 'Sex'))
            ->withEntry('age', array_get(ref('row'), 'Age'))
            ->withEntry('fare', array_get(ref('row'), 'Fare'))";
    }

    public function test_a_median_answers_beside_the_average_it_disagrees_with(): void
    {
        $rows = $this->rows($this->read() . "->groupBy(ref('sex'))
               ->aggregate(average(ref('age')->as('mean_age')), median(ref('age')))
               ->fetch()");

        $bySex = [];

        foreach ($rows as $row) {
            $bySex[$row['sex']] = [$row['mean_age'], $row['age_median']];
        }

        // The two numbers a curation would publish for the same column. They
        // are close here and would not be on fares, which is the point of
        // being able to ask for both.
        self::assertSame([25.67, 25.0], $bySex['female']);
        self::assertSame([21.33, 22.0], $bySex['male']);
    }

    public function test_an_age_nobody_recorded_is_skipped_rather_than_counted_as_zero(): void
    {
        $rows = $this->rows($this->read() . "->groupBy(ref('sex'))->aggregate(median(ref('age')))->fetch()");

        $male = null;

        foreach ($rows as $row) {
            if ($row['sex'] !== 'male') {
                continue;
            }

            $male = $row['age_median'];
        }

        // Three men reported 22, 2 and 40, and a fourth reported nothing. Read
        // as a zero the median would be 12; skipped, it is 22.
        self::assertSame(22.0, $male);
    }

    public function test_a_group_too_small_for_a_deviation_answers_null(): void
    {
        // One row per passenger, which is the smallest group there is.
        $rows = $this->rows($this->read() . "->groupBy(ref('name'))
               ->aggregate(median(ref('age')), stddev(ref('age')), percentile(ref('age'), 90))
               ->fetch()");

        self::assertNotSame([], $rows);

        foreach ($rows as $row) {
            self::assertNull($row['age_stddev']);
            self::assertNull($row['age_percentile']);
        }

        // And the median, which is defined over one value, is not null for the
        // passengers whose age this corpus records.
        self::assertNotNull($rows[0]['age_median']);
    }

    public function test_the_column_a_statistic_writes_is_the_one_a_later_sort_can_name(): void
    {
        $rows = $this->rows($this->read() . "->groupBy(ref('sex'))
               ->aggregate(median(ref('age')), percentile(ref('fare')->as('fare_p90'), 90))
               ->sortBy(ref('age_median')->desc())
               ->fetch()");

        // `age_median` the way Flow writes `age_avg`, and the alias where one
        // was given — a percentile that kept the default name would not say
        // which percentile it was.
        self::assertSame(['sex', 'age_median', 'fare_p90'], \array_keys($rows[0]));
        self::assertGreaterThanOrEqual($rows[1]['age_median'], $rows[0]['age_median']);
    }

    public function test_a_percentile_says_what_it_needs_and_what_it_will_not_take(): void
    {
        foreach ([
            "percentile(ref('age'))" => 'a column and a percentage',
            "percentile(ref('age'), 150)" => 'between 0 and 100',
            "percentile(ref('age'), 'ninety')" => 'as its second argument',
            "median(ref('age'), 90)" => 'takes one column',
        ] as $aggregation => $expected) {
            try {
                $this->rows($this->read() . "->groupBy(ref('sex'))->aggregate({$aggregation})->fetch()");
                self::fail("expected {$aggregation} to be refused");
            } catch (ParseError $error) {
                self::assertStringContainsString($expected, $error->getMessage());
            }
        }
    }

    public function test_a_bare_statistic_in_with_entry_is_refused_with_both_ways_out(): void
    {
        // A bare aggregation answers one row per group, so it cannot fill a
        // column — and the message says the two things that can: put it in
        // aggregate(), or give it a window.
        try {
            $this->rows($this->read() . "->withEntry('typical_age', median(ref('age')))->fetch()");
            self::fail('an aggregation is not a value per row');
        } catch (ParseError $error) {
            self::assertStringContainsString('one row per group', $error->getMessage());
            self::assertStringContainsString('aggregate()', $error->getMessage());
            self::assertStringContainsString('over(window()', $error->getMessage());
        }
    }

    public function test_the_same_statistic_with_a_window_fills_a_column(): void
    {
        // The thing the message points at, and the reason a curation no longer
        // leaves the query language to fill a missing value: the median of the
        // row's own status group, beside the row, with the original left
        // holding its null so a guess stays distinguishable from a measurement.
        $rows = $this->rows(
            $this->read()
            . "->withEntry('title', regex_replace(lit('/^[^,]*,\s*([^.]+)\..*\$/'), lit('\$1'), ref('name')))"
            . "->withEntry('typical_age', median(ref('age'))->over(window()->partitionBy(ref('title'))))"
            . "->withEntry('age_filled', coalesce(ref('age'), ref('typical_age')))"
            . "->select(ref('name'), ref('title'), ref('age'), ref('typical_age'), ref('age_filled'))"
            . '->fetch()',
        );

        $missing = null;

        foreach ($rows as $row) {
            if ($row['age'] !== null) {
                continue;
            }

            $missing = $row;
        }

        self::assertNotNull($missing, 'the corpus records no age for passenger 6');
        // The other `Mr.` in the fixture reported 22, so that is what this one
        // is given — and its own `age` is still null.
        self::assertSame(22.0, $missing['typical_age']);
        self::assertSame(22.0, $missing['age_filled']);
        self::assertNull($missing['age']);
    }
}
