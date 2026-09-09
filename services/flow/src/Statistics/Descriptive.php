<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Statistics;

use HiFolks\Statistics\Stat;

/**
 * What a query may ask of a column of numbers, beyond what Flow ships.
 *
 * Flow's aggregations answer "how many" and "how much" and miss the half of
 * `describe()` that says how a column is *distributed*. A median is not an
 * average when a handful of first-class fares are ten times everybody else's.
 *
 * The arithmetic is [hi-folks/statistics][lib] rather than written here: MIT, no
 * dependencies, the Python `statistics` module's definitions, PHPStan level 8.
 * A hand-rolled quantile with an off-by-one in its interpolation is a wrong
 * number that looks right.
 *
 * **Not enough data is not zero.** Each of these is undefined below some number
 * of values, and a query gets **null** rather than 0 — a group of one passenger
 * has no sample deviation, and 0 would be a claim nobody made. Flow's own
 * `average()` answers 0 for an empty group, which is its choice and not one to
 * copy: a curation is read later by somebody who was not there when it ran.
 *
 * [lib]: https://github.com/Hi-Folks/statistics
 */
enum Descriptive: string
{
    case Median = 'median';
    case StandardDeviation = 'stddev';
    case Variance = 'variance';
    case Percentile = 'percentile';

    /**
     * How many reported values the statistic needs before it means anything.
     *
     * One for a median, which is a value the data actually has. Two for the
     * rest: a sample deviation divides by `n - 1`, a sample variance is its
     * square, and a percentile interpolates *between* two points.
     */
    public function needs(): int
    {
        return match ($this) {
            self::Median => 1,
            self::StandardDeviation, self::Variance, self::Percentile => 2,
        };
    }

    /** @return list<string> What a query may write, for a "did you mean". */
    public static function names(): array
    {
        return \array_map(static fn(self $statistic): string => $statistic->value, self::cases());
    }

    /** Whether the query has to say which percentage it wants. */
    public function takesAPercentage(): bool
    {
        return $this === self::Percentile;
    }

    /**
     * The answer, or null when this group did not report enough numbers.
     *
     * The count is checked here rather than caught from the library, because
     * the library's exception is how it reports a precondition and a caught
     * exception is a slower way of asking the same question — and because
     * `count()` is the whole of the precondition it documents.
     *
     * @param list<float> $values
     */
    public function of(array $values, float $percentage = 50.0): ?float
    {
        if (\count($values) < $this->needs()) {
            return null;
        }

        $answer = match ($this) {
            self::Median => Stat::median($values),
            // The sample forms, not the population ones: a curation is a
            // sample of a corpus by construction — it read a window, a limit,
            // or a hub's first hundred rows.
            self::StandardDeviation => Stat::stdev($values),
            self::Variance => Stat::variance($values),
            self::Percentile => Stat::percentile($values, $percentage),
        };

        return \is_numeric($answer) ? (float) $answer : null;
    }
}
