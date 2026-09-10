<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dsl;

use Aiwatcher\Flow\Dsl\Admission;
use Aiwatcher\Flow\Dsl\Frame;
use Aiwatcher\Flow\Dsl\Values;
use Flow\ETL\DataFrame;
use PHPUnit\Framework\TestCase;

use function Flow\ETL\DSL\count as flow_count;
use function Flow\ETL\DSL\ref;

/**
 * The rule that decides what a query may reach, asked about the one thing
 * reflection does not report the same way on every PHP this service supports.
 *
 * Flow writes almost every `DataFrame` method `: self`. PHP 8.5 resolves that
 * to `Flow\ETL\DataFrame` in `ReflectionNamedType::getName()`; 8.3 and 8.4
 * report the word `self`. Matching the written word therefore admitted 24
 * steps on one interpreter and one on another — the same source, the same lock
 * file, two different query languages, and green on a laptop while CI, which
 * stands on the declared floor, failed 41 tests.
 *
 * These assert the resolution rather than a version, because that is the part
 * a single interpreter can prove.
 */
final class AdmissionTest extends TestCase
{
    public function test_a_relative_return_type_names_the_class_it_stands_for(): void
    {
        $method = (new \ReflectionClass(DataFrame::class))->getMethod('withEntry');

        // What 8.3 writes and 8.5 resolves. Either way, this is the answer.
        self::assertSame(DataFrame::class, Admission::returned($method));
    }

    public function test_the_steps_a_pipeline_may_take_do_not_depend_on_how_flow_spelled_them(): void
    {
        // Every one of these is declared `: self`. Reading the written word
        // left `groupBy` — which names its return type in full — as the only
        // step in the language.
        foreach (['withEntry', 'filter', 'aggregate', 'select', 'limit', 'join'] as $step) {
            self::assertTrue(Frame::isStep($step), \sprintf('->%s() is a step', $step));
        }

        self::assertGreaterThan(20, \count(Frame::methods()));
    }

    public function test_a_method_that_hands_back_its_own_class_keeps_the_value_a_value(): void
    {
        // `->as()` on a reference and `->over()` on an aggregation are both
        // fluent chains, and `Statistic::over(): static` is this service's own
        // class rather than one of Flow's namespaces — so the rule is "it
        // returns what it was called on", not a list of two spellings.
        self::assertContains('as', Values::names(ref('age')));
        self::assertContains('over', Values::names(flow_count(ref('age'))));
    }
}
