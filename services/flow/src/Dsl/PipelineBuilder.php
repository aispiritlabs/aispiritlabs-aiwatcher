<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dataset\Dataset;
use Flow\ETL\DataFrame;
use Flow\ETL\DataFrame\GroupedDataFrame;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Row\EntryReference;
use Flow\ETL\Row\Reference;

use function Flow\ETL\DSL\array_expand;
use function Flow\ETL\DSL\array_get;
use function Flow\ETL\DSL\average;
use function Flow\ETL\DSL\between;
use function Flow\ETL\DSL\cast;
use function Flow\ETL\DSL\coalesce;
use function Flow\ETL\DSL\collect;
use function Flow\ETL\DSL\collect_unique;
use function Flow\ETL\DSL\concat;
use function Flow\ETL\DSL\concat_ws;
use function Flow\ETL\DSL\count;
use function Flow\ETL\DSL\exists;
use function Flow\ETL\DSL\first;
use function Flow\ETL\DSL\hash;
use function Flow\ETL\DSL\identical;
use function Flow\ETL\DSL\last;
use function Flow\ETL\DSL\lit;
use function Flow\ETL\DSL\lower;
use function Flow\ETL\DSL\max;
use function Flow\ETL\DSL\min;
use function Flow\ETL\DSL\not;
use function Flow\ETL\DSL\optional;
use function Flow\ETL\DSL\ref;
use function Flow\ETL\DSL\regex_match;
use function Flow\ETL\DSL\round;
use function Flow\ETL\DSL\size;
use function Flow\ETL\DSL\split;
use function Flow\ETL\DSL\sum;
use function Flow\ETL\DSL\upper;
use function Flow\ETL\DSL\when;

/**
 * Turns a parsed query into a Flow pipeline.
 *
 * ## The one rule
 *
 * Every name that came from the query is resolved through an explicit `match`,
 * never through a variable function call. `$name(...)` after a whitelist check
 * would probably be safe; a `match` is safe without the "probably", and stays
 * safe if the whitelist check is ever refactored badly. This file is the only
 * place a name in the query text becomes a call, so it is worth the verbosity.
 *
 * ## Column checking
 *
 * References are validated against the dataset's declared columns as the
 * pipeline is built, and the set grows as `withEntry` and `->as()` add names.
 * That is what turns Flow's runtime "entry does not exist" into an error that
 * arrives before anything runs and says what to write instead.
 */
final class PipelineBuilder
{
    /** @var array<string, true> Columns a reference may name at this point. */
    private array $known = [];

    private bool $truncate = true;

    private ?Dataset $dataset = null;

    public function __construct(
        private readonly Catalog $catalog,
        /**
         * The panel's time window, in seconds, or null for everything.
         *
         * Not part of the language on purpose: a query says *what* to read,
         * and every list in the panel is already scoped by one control. A
         * `->window(900)` step would be a second way to say it, and the two
         * would disagree the first time somebody set both.
         */
        private readonly ?int $windowSeconds = null,
    ) {}

    public function build(Query $query): Plan
    {
        $steps = $query->steps;
        $first = $steps[0];

        if ($first->name !== 'read') {
            throw new ParseError('A query starts by reading a dataset: ->read(default).', $first->column);
        }

        $frame = $this->read($first);

        foreach (\array_slice($steps, 1) as $step) {
            $frame = $this->apply($frame, $step);
        }

        if ($frame instanceof GroupedDataFrame) {
            throw new ParseError(
                'groupBy() has to be followed by aggregate(), e.g. '
                . 'aggregate(count(ref(\'run_id\')->as(\'runs\'))). '
                . 'Grouping on its own produces nothing to show.',
            );
        }

        return new Plan($frame, $this->dataset, $this->truncate);
    }

    private function read(Step $step): DataFrame
    {
        $name = null;
        $run = null;

        foreach ($step->args as $argument) {
            if ($argument->name === 'run') {
                $run = $this->scalar($argument->value, 'run');

                continue;
            }

            if ($argument->name !== null) {
                throw new ParseError(
                    \sprintf('read() has no argument "%s". It takes a dataset, and optionally run:.', $argument->name),
                    $argument->value->column(),
                );
            }

            $value = $argument->value;
            $name = match (true) {
                $value instanceof Bareword => $value->name,
                $value instanceof Literal && \is_string($value->value) => $value->value,
                default => throw new ParseError('read() takes a dataset name.', $value->column()),
            };
        }

        if ($name === null) {
            throw new ParseError('read() needs a dataset. Try read(default).', $step->column);
        }

        $dataset = $this->catalog->resolve($name);

        if ($dataset === null) {
            throw new ParseError(
                \sprintf(
                    'There is no dataset "%s". Available: %s (and "default", which is "runs").',
                    $name,
                    \implode(', ', \array_keys($this->catalog->all())),
                ),
                $step->column,
            );
        }

        if ($dataset->requiresRun && ($run === null || $run === '')) {
            throw new ParseError(
                \sprintf(
                    'The "%s" dataset is per run: write read(%s, run: \'run-1\'). '
                    . 'Without a run there is no route to call, and walking every run instead '
                    . 'would be a request per run across the whole retention window.',
                    $dataset->name,
                    $dataset->name,
                ),
                $step->column,
            );
        }

        $this->dataset = $dataset;
        $this->known = \array_fill_keys(\array_keys($dataset->columns), true);

        return $this->catalog->open($dataset, \is_string($run) ? $run : null, $this->windowSeconds);
    }

    /**
     * One step.
     *
     * `groupBy` returns a `GroupedDataFrame`, on which the only legal move is
     * `aggregate`. Threading that through the signature rather than hiding it
     * is what lets a dangling `groupBy` be a sentence-long error instead of a
     * type error from inside Flow.
     */
    private function apply(DataFrame|GroupedDataFrame $frame, Step $step): DataFrame|GroupedDataFrame
    {
        if ($frame instanceof GroupedDataFrame && $step->name !== 'aggregate') {
            throw new ParseError(
                \sprintf('After groupBy() the only step is aggregate(), not %s().', $step->name),
                $step->column,
            );
        }

        \assert(
            $frame instanceof DataFrame || $step->name === 'aggregate',
            'a grouped frame reached a step other than aggregate',
        );

        if ($step->name === 'aggregate') {
            return $this->aggregate($frame, $step);
        }

        \assert($frame instanceof DataFrame, 'aggregate is the only step that takes a grouped frame');

        return match ($step->name) {
            'select' => $frame->select(...$this->references($step)),
            'drop' => $frame->drop(...$this->references($step)),
            'groupBy' => $frame->groupBy(...$this->references($step)),
            'sortBy' => $frame->sortBy(...$this->references($step)),
            'filter' => $frame->filter($this->scalarFunction($step)),
            'limit' => $frame->limit($this->limit($step)),
            'withEntry' => $this->withEntry($frame, $step),
            'write' => $this->write($step, $frame),
            // The pipeline is always finished by the caller, with the row cap
            // applied. A trailing run() or fetch() is accepted so a query
            // written for the CLI pastes in unchanged, and does nothing here.
            'run', 'fetch' => $frame,
            default => throw new ParseError(\sprintf('"%s" is not a pipeline step.', $step->name), $step->column),
        };
    }

    private function withEntry(DataFrame $frame, Step $step): DataFrame
    {
        if (\count($step->args) !== 2) {
            throw new ParseError('withEntry() takes a name and a value.', $step->column);
        }

        $name = $this->scalar($step->args[0]->value, 'withEntry name');

        if (!\is_string($name)) {
            throw new ParseError('withEntry() takes a name and a value.', $step->args[0]->value->column());
        }

        $value = $this->node($step->args[1]->value);

        if (!$value instanceof ScalarFunction) {
            throw new ParseError('withEntry() needs a value built from ref(), lit() or a function.', $step->column);
        }

        // The new column is nameable from here on, which is what makes
        // withEntry('agent', array_expand(ref('agents'))) followed by
        // groupBy(ref('agent')) work.
        $this->known[$name] = true;

        return $frame->withEntry($name, $value);
    }

    private function aggregate(DataFrame|GroupedDataFrame $frame, Step $step): DataFrame
    {
        if ($step->args === []) {
            throw new ParseError('aggregate() needs at least one aggregation.', $step->column);
        }

        $aggregations = [];
        $produced = [];

        foreach ($step->args as $argument) {
            $call = $argument->value;

            if (!$call instanceof Call || !Whitelist::isAggregation($call->name)) {
                throw new ParseError(
                    \sprintf('aggregate() takes aggregations: %s.', \implode(', ', Whitelist::AGGREGATIONS)),
                    $argument->value->column(),
                );
            }

            $aggregations[] = $this->node($call);
            $produced[] = $this->aggregateOutputName($call);
        }

        // After aggregating, only the group keys and the aggregation outputs
        // exist. Keeping the pre-aggregation columns "known" would let a later
        // sortBy name a column that is no longer there.
        $keys = $this->known['__group_keys__'] ?? null;
        $this->known = \array_fill_keys(\array_merge(\is_array($keys) ? $keys : [], $produced), true);

        return $frame->aggregate(...$aggregations);
    }

    /**
     * What column an aggregation writes.
     *
     * Flow names it after the reference plus the function — `run_id_count` —
     * unless the reference carries an alias. Mirroring that here is what lets
     * `sortBy(ref('runs'))` after `count(ref('run_id')->as('runs'))` validate.
     */
    private function aggregateOutputName(Call $call): string
    {
        $inner = $call->args[0]->value ?? null;

        if (!$inner instanceof Call) {
            return '_' . $call->name;
        }

        if ($inner->alias !== null) {
            return $inner->alias;
        }

        $column = $inner->args[0]->value ?? null;
        $base = $column instanceof Literal && \is_string($column->value) ? $column->value : 'value';

        return $base . '_' . $call->name;
    }

    private function write(Step $step, DataFrame $frame): DataFrame
    {
        foreach ($step->args as $argument) {
            $sink = $argument->value;

            if (!$sink instanceof Call || !Whitelist::isSink($sink->name)) {
                throw new ParseError(
                    \sprintf('write() takes one of: %s.', \implode(', ', Whitelist::SINKS)),
                    $argument->value->column(),
                );
            }

            foreach ($sink->args as $option) {
                if ($option->name !== 'truncate') {
                    continue;
                }

                $this->truncate = (bool) $this->scalar($option->value, 'truncate');
            }
        }

        // Every sink means the same thing: give the rows back. Which one was
        // written only changes whether the panel shortens long cells.
        return $frame;
    }

    private function limit(Step $step): int
    {
        $value = $step->args[0]->value ?? null;

        if (!$value instanceof Literal || !\is_int($value->value) || $value->value < 1) {
            throw new ParseError('limit() takes a positive whole number.', $step->column);
        }

        return $value->value;
    }

    /** @return list<Reference> */
    private function references(Step $step): array
    {
        if ($step->args === []) {
            throw new ParseError(\sprintf('%s() needs at least one column.', $step->name), $step->column);
        }

        $references = [];

        foreach ($step->args as $argument) {
            $node = $this->node($argument->value);

            if (!$node instanceof Reference) {
                throw new ParseError(
                    \sprintf('%s() takes columns, written as ref(\'name\').', $step->name),
                    $argument->value->column(),
                );
            }

            $references[] = $node;
        }

        if ($step->name === 'groupBy') {
            // Remembered so `aggregate` knows which columns survive it.
            $this->known['__group_keys__'] = \array_map(
                static fn(Reference $reference): string => $reference->name(),
                $references,
            );
        }

        return $references;
    }

    private function scalarFunction(Step $step): ScalarFunction
    {
        $node = $this->node(
            $step->args[0]->value ?? throw new ParseError('filter() needs a condition.', $step->column),
        );

        if (!$node instanceof ScalarFunction) {
            throw new ParseError(
                'filter() needs a condition, e.g. equal(ref(\'status\'), lit(\'failed\')).',
                $step->column,
            );
        }

        return $node;
    }

    private function scalar(Node $node, string $what): string|int|float|bool|null
    {
        if (!$node instanceof Literal) {
            throw new ParseError(\sprintf('%s must be a literal value.', $what), $node->column());
        }

        return $node->value;
    }

    /**
     * One value, resolved.
     *
     * The `match` is the security boundary: a name from the query only ever
     * selects a branch here, and never becomes a callable.
     */
    private function node(Node $node): mixed
    {
        if ($node instanceof Literal) {
            return $node->value;
        }

        if ($node instanceof Bareword) {
            throw new ParseError(
                \sprintf('"%s" means nothing here. A column is written ref(\'%s\').', $node->name, $node->name),
                $node->column(),
            );
        }

        \assert($node instanceof Call, 'a node is a literal, a bareword or a call');

        $args = \array_map(fn(Argument $argument): mixed => $this->node($argument->value), $node->args);

        $value = match ($node->name) {
            'ref', 'col' => $this->referenceOrComparison($node),
            'lit' => lit($args[0] ?? null),
            'count' => count(...$this->fns($node, $args)),
            'sum' => sum(...$this->fns($node, $args)),
            'average' => average(...$this->fns($node, $args)),
            'min' => min(...$this->fns($node, $args)),
            'max' => max(...$this->fns($node, $args)),
            'first' => first(...$this->fns($node, $args)),
            'last' => last(...$this->fns($node, $args)),
            'collect' => collect(...$this->fns($node, $args)),
            'collect_unique' => collect_unique(...$this->fns($node, $args)),
            'array_get' => array_get($args[0], (string) $args[1]),
            'array_expand' => array_expand($args[0]),
            'concat' => concat(...$args),
            'concat_ws' => concat_ws((string) $args[0], ...\array_slice($args, 1)),
            'lower' => lower($args[0]),
            'upper' => upper($args[0]),
            'cast' => cast($args[0], (string) $args[1]),
            'coalesce' => coalesce(...$args),
            'size' => size($args[0]),
            'round' => round($args[0], $args[1] ?? 0),
            'when' => when($args[0], $args[1], $args[2] ?? null),
            'exists' => exists($args[0]),
            'not' => not($args[0]),
            'between' => between($args[0], $args[1], $args[2]),
            'regex_match' => regex_match($args[0], $args[1]),
            'split' => split($args[0], (string) $args[1]),
            'hash' => hash($args[0]),
            'identical' => identical($args[0], $args[1]),
            'optional' => optional($args[0]),
            // Sinks are handled by `write`; reaching here means one was used as
            // a value, which is not a thing.
            'to_output', 'to_array', 'to_memory' => throw new ParseError(
                \sprintf('%s() belongs inside write().', $node->name),
                $node->column(),
            ),
            default => throw new ParseError(
                \sprintf('"%s" is not part of the query language.', $node->name),
                $node->column(),
            ),
        };

        // `ref`/`col` already applied their own chain above; applying it twice
        // would compare the comparison.
        return \in_array($node->name, ['ref', 'col'], true) ? $value : $this->compare($value, $node);
    }

    /**
     * Apply `->equals(...)`, `->greaterThan(...)` and friends.
     *
     * Same rule as everywhere else: the method name selects a branch, never a
     * callable. Flow puts these on the value because the standalone `equal()`
     * compares two columns — comparing a column to a literal is the chained
     * form, and it is the one people actually want.
     */
    private function compare(mixed $value, Call $node): mixed
    {
        foreach ($node->chain as $method) {
            if (!$value instanceof ScalarFunction) {
                throw new ParseError(
                    \sprintf('->%s(...) needs a value, e.g. ref(\'…\')->%s(…).', $method->name, $method->name),
                    $method->column,
                );
            }

            $argument = isset($method->args[0]) ? $this->node($method->args[0]->value) : null;

            $value = match ($method->name) {
                'equals' => $value->equals($argument),
                'notEquals' => $value->notEquals($argument),
                'same' => $value->same($argument),
                'notSame' => $value->notSame($argument),
                'greaterThan' => $value->greaterThan($argument),
                'greaterThanEqual' => $value->greaterThanEqual($argument),
                'lessThan' => $value->lessThan($argument),
                'lessThanEqual' => $value->lessThanEqual($argument),
                'isIn' => $value->isIn($this->haystack($argument, $method)),
                'contains' => $value->contains($this->text($argument, $method)),
                'startsWith' => $value->startsWith($this->text($argument, $method)),
                'endsWith' => $value->endsWith($this->text($argument, $method)),
                'isNull' => $value->isNull(),
                'isNotNull' => $value->isNotNull(),
                'isTrue' => $value->isTrue(),
                'isFalse' => $value->isFalse(),
                'isEmpty' => $value->isEmpty(),
                default => throw new ParseError(
                    \sprintf('"%s" is not something a value supports.', $method->name),
                    $method->column,
                ),
            };
        }

        return $value;
    }

    private function text(mixed $value, Step $method): ScalarFunction|string
    {
        if ($value instanceof ScalarFunction || \is_string($value)) {
            return $value;
        }

        throw new ParseError(\sprintf('->%s() takes a string.', $method->name), $method->column);
    }

    /** @return ScalarFunction|array<int, mixed> */
    private function haystack(mixed $value, Step $method): ScalarFunction|array
    {
        if ($value instanceof ScalarFunction || \is_array($value)) {
            return $value;
        }

        throw new ParseError(
            \sprintf('->%s() takes a column holding a list, e.g. ref(\'agents\').', $method->name),
            $method->column,
        );
    }

    /**
     * The arguments of an aggregation, which Flow types as references.
     *
     * @param list<mixed> $args
     *
     * @return list<Reference>
     */
    private function fns(Call $node, array $args): array
    {
        foreach ($args as $argument) {
            if (!$argument instanceof Reference) {
                throw new ParseError(
                    \sprintf('%s() takes a column, written as ref(\'name\').', $node->name),
                    $node->column(),
                );
            }
        }

        /** @var list<Reference> $args */
        return $args;
    }

    private function reference(Call $node): Reference
    {
        $name = $node->args[0]->value ?? null;

        if (!$name instanceof Literal || !\is_string($name->value)) {
            throw new ParseError('ref() takes a column name.', $node->column());
        }

        if ($this->dataset !== null && !isset($this->known[$name->value])) {
            throw new ParseError($this->dataset->explainUnknownColumn($name->value), $name->column());
        }

        $reference = ref($name->value);

        if ($node->alias !== null) {
            $this->known[$node->alias] = true;
            $reference = $reference->as($node->alias);
        }

        if ($node->order !== null && $reference instanceof EntryReference) {
            $reference = $node->order === 'desc' ? $reference->desc() : $reference->asc();
        }

        return $reference;
    }

    /** A reference with comparisons chained on is no longer a plain reference. */
    private function referenceOrComparison(Call $node): mixed
    {
        $reference = $this->reference($node);

        return $node->chain === [] ? $reference : $this->compare($reference, $node);
    }
}
