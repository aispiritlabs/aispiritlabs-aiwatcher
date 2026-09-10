<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Statistics;

use Flow\ETL\Exception\InvalidArgumentException;
use Flow\ETL\Exception\RuntimeException;
use Flow\ETL\FlowContext;
use Flow\ETL\Function\AggregatingFunction;
use Flow\ETL\Function\WindowFunction;
use Flow\ETL\Row;
use Flow\ETL\Row\Entry;
use Flow\ETL\Row\EntryFactory;
use Flow\ETL\Row\Reference;
use Flow\ETL\Window;
use Flow\ETL\Window\WindowContext;

use function Flow\ETL\DSL\float_entry;

/**
 * One [`Descriptive`] statistic over one column, as an aggregation Flow can run.
 *
 * One of the two things in this service that add a name to the query language
 * which is **not** a Flow function — the other is [`Dsl\Sink`].
 * [`Dsl\Registry`] admits Flow's own namespace by return type and would never
 * admit this, which is correct: the rule there is about a vocabulary somebody
 * else maintains. These four are ours, and what admits them is
 * [`Descriptive::tryFrom`] — the enum that is also their implementation, so the
 * vocabulary cannot grow without a `match` arm to go with it.
 * `Dsl\PipelineBuilder` constructs them the same explicit way every other name
 * goes through, so ADR_0008's rule is unchanged: a name from a query selects a
 * branch, and never becomes a callable.
 *
 * Nothing lists this as an aggregation, either. `aggregate()` takes what
 * implements Flow's `AggregatingFunction`, which this does — a list of names
 * beside the arms that build them is the same enumeration one layer down.
 *
 * Flow clones an aggregator per group (`GroupBy\Aggregators::cloned`), and a
 * PHP array is copied on clone — which is what keeps one group's values out of
 * the next one's answer.
 */
final class Statistic implements AggregatingFunction, WindowFunction
{
    /** @var list<float> */
    private array $values = [];

    private ?Window $frame = null;

    public function __construct(
        private readonly Reference $ref,
        private readonly Descriptive $of,
        private readonly float $percentage = 50.0,
    ) {}

    public function aggregate(Row $row, FlowContext $context): void
    {
        try {
            /** @var mixed $value */
            $value = $row->valueOf($this->ref);
        } catch (InvalidArgumentException $error) {
            $context
                ->functions()
                ->invalidResult(new InvalidArgumentException($this->of->value . ' error: ' . $error->getMessage()));

            return;
        }

        // The same test `average()` applies, so a column of numbers written as
        // strings — which is how more than one hub sends them — is read the
        // same way by both. A null contributes nothing rather than a zero.
        if (!\is_numeric($value)) {
            return;
        }

        $this->values[] = (float) $value;
    }

    /** @return list<Reference> */
    public function references(): array
    {
        return [$this->ref];
    }

    /**
     * The same statistic, answered once per row instead of once per group.
     *
     * This is what a query needs to put a group's median *beside* the rows it
     * was taken over — `median(ref('age'))->over(window()->partitionBy(ref('status')))`
     * — and it is the reason a curation no longer has to leave the query
     * language to fill a missing value from its group. Flow computes the
     * partition and hands over the frame; the arithmetic is the same
     * [`Descriptive`] the aggregating half uses, over the same skip-nulls rule,
     * with the same null for a frame too small to have an answer.
     */
    public function apply(WindowContext $window): mixed
    {
        $values = [];

        foreach ($window->frame() as $row) {
            /** @var mixed $value */
            $value = $row->valueOf($this->ref);

            if (!\is_numeric($value)) {
                continue;
            }

            $values[] = (float) $value;
        }

        return $this->of->of($values, $this->percentage);
    }

    public function over(Window $window): static
    {
        $this->frame = $window;

        return $this;
    }

    public function window(): Window
    {
        if ($this->frame === null) {
            throw new RuntimeException('Window function "' . $this->toString() . '" requires an OVER clause.');
        }

        return $this->frame;
    }

    public function result(EntryFactory $entryFactory): Entry
    {
        if (!$this->ref->hasAlias()) {
            // `age_median`, the way Flow names `age_avg` — and the same name
            // `PipelineBuilder::aggregateOutputName` predicts, which is what
            // lets a later sortBy() name the column this writes.
            $this->ref->as($this->ref->to() . '_' . $this->of->value);
        }

        return float_entry($this->ref->name(), $this->of->of($this->values, $this->percentage));
    }

    public function toString(): string
    {
        return $this->of->value . '()';
    }
}
