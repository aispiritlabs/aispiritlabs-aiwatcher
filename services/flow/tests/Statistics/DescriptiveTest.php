<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Statistics;

use Aiwatcher\Flow\Statistics\Descriptive;
use PHPUnit\Framework\TestCase;

/**
 * The four statistics a query may ask for, and the two rules around them.
 *
 * The arithmetic belongs to hi-folks/statistics and is its problem; what is
 * this repository's problem is which definition was chosen and what happens
 * when a group is too small to have one.
 */
final class DescriptiveTest extends TestCase
{
    public function test_a_median_is_the_middle_value_rather_than_the_average(): void
    {
        // The whole reason for having it: one first-class fare among steerage
        // ones moves a mean and does not move a median.
        self::assertSame(1.0, Descriptive::Median->of([1.0, 1.0, 1.0, 10.0]));
    }

    public function test_a_median_of_one_value_is_that_value(): void
    {
        self::assertSame(7.0, Descriptive::Median->of([7.0]));
    }

    public function test_a_statistic_over_too_few_numbers_is_null_rather_than_zero(): void
    {
        // A group of one passenger has no sample deviation. Answering 0 would
        // be publishing a claim nobody made — and a dataset version is read
        // later by somebody who was not there when it ran.
        self::assertNull(Descriptive::StandardDeviation->of([5.0]));
        self::assertNull(Descriptive::Variance->of([5.0]));
        self::assertNull(Descriptive::Percentile->of([5.0], 90.0));
        self::assertNull(Descriptive::Median->of([]));
    }

    public function test_the_deviation_is_the_sample_one_rather_than_the_population_one(): void
    {
        // A curation is a sample by construction: it read a window, a limit,
        // or a hub's first hundred rows. The population form of this data is
        // exactly 2.0, so this assertion is the choice rather than a rounding.
        self::assertEqualsWithDelta(
            2.1381,
            Descriptive::StandardDeviation->of([2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]) ?? 0.0,
            0.0001,
        );
    }

    public function test_a_percentile_interpolates_between_the_values_it_falls_between(): void
    {
        self::assertSame(15.0, Descriptive::Percentile->of([10.0, 20.0], 50.0));
    }

    public function test_only_a_percentile_is_asked_which_percentage(): void
    {
        // What the builder checks a second argument for, in one place.
        self::assertTrue(Descriptive::Percentile->takesAPercentage());
        self::assertFalse(Descriptive::Median->takesAPercentage());
    }
}
