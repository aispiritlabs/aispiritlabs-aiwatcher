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

use function Flow\ETL\DSL\all;
use function Flow\ETL\DSL\any;
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

    /** The request window, overridden by read(..., period:) when the script pins one. */
    private ?int $effectiveWindowSeconds;

    public function __construct(
        private readonly Catalog $catalog,
        /** The panel's fallback time window, in seconds, or null for everything. */
        ?int $windowSeconds = null,
    ) {
        $this->effectiveWindowSeconds = $windowSeconds;
    }

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

        return new Plan($frame, $this->dataset, $this->truncate, $this->effectiveWindowSeconds);
    }

    private function read(Step $step): DataFrame
    {
        $name = null;
        $run = null;
        $periodWasSet = false;
        /** @var array<string, array{value: string, column: int}> */
        $named = [];

        foreach ($step->args as $argument) {
            if ($argument->name === 'run') {
                $run = $this->scalar($argument->value, 'run');

                continue;
            }

            if ($argument->name === 'period') {
                if ($periodWasSet) {
                    throw new ParseError('read() takes period: only once.', $argument->value->column());
                }

                $this->effectiveWindowSeconds = $this->period($argument->value);
                $periodWasSet = true;

                continue;
            }

            if ($argument->name !== null) {
                // Held rather than checked here: which arguments are legal
                // depends on the dataset, and the dataset name may come after
                // them. `read(q: 'plans', hub_datasets)` is odd but valid,
                // and refusing it would be refusing it for the wrong reason.
                $named[$argument->name] = [
                    'value' => (string) $this->scalar($argument->value, $argument->name),
                    'column' => $argument->value->column(),
                ];

                continue;
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

        if ($periodWasSet && !$dataset->windowed) {
            throw new ParseError(
                \sprintf('Dataset "%s" does not take a period. It is already bounded to one run.', $dataset->name),
                $step->column,
            );
        }

        $arguments = $this->readArguments($dataset, $named, $step->column);

        $this->dataset = $dataset;
        $this->known = \array_fill_keys(\array_keys($dataset->columns), true);

        return $this->catalog->open(
            $dataset,
            \is_string($run) ? $run : null,
            $this->effectiveWindowSeconds,
            $arguments,
        );
    }

    /**
     * The named arguments a `read()` may carry into this dataset's route.
     *
     * Checked against what the dataset declares rather than forwarded. The
     * aiwatcher API rejects unknown query parameters, so an undeclared one
     * would come back as a 400 about the whole request with nothing pointing
     * at the word that caused it — and a *misspelled value* would come back as
     * an empty result, which reads as "no matches" rather than as a typo.
     *
     * @param  array<string, array{value: string, column: int}> $named
     * @return array<string, string>
     */
    private function readArguments(Dataset $dataset, array $named, int $column): array
    {
        $arguments = [];

        foreach ($named as $key => $entry) {
            $parameter = $dataset->parameters[$key] ?? null;

            if ($parameter === null) {
                throw new ParseError($dataset->explainUnknownParameter($key), $entry['column']);
            }

            if (!$parameter->accepts($entry['value'])) {
                throw new ParseError(
                    \sprintf(
                        '%s: takes one of %s, not "%s".',
                        $key,
                        \implode(', ', \array_map(
                            static fn(string $value): string => "'" . $value . "'",
                            $parameter->values,
                        )),
                        $entry['value'],
                    ),
                    $entry['column'],
                );
            }

            $arguments[$key] = $entry['value'];
        }

        foreach ($dataset->parameters as $key => $parameter) {
            if ($parameter->required && ($arguments[$key] ?? '') === '') {
                throw new ParseError(
                    \sprintf(
                        'The "%s" dataset needs %s: — %s Write read(%s, %s: \'…\').',
                        $dataset->name,
                        $key,
                        $parameter->description,
                        $dataset->name,
                        $key,
                    ),
                    $column,
                );
            }
        }

        return $arguments;
    }

    /**
     * A reproducible relative period embedded in the Flow script.
     *
     * Examples: period: '15m', '6h', '7d', '2w', 'all', or 3600 seconds.
     */
    private function period(Node $node): ?int
    {
        $value = $this->scalar($node, 'period');

        if ($value === 'all') {
            return null;
        }

        if (\is_int($value)) {
            if ($value > 0) {
                return $value;
            }

            throw new ParseError('period: seconds must be a positive whole number.', $node->column());
        }

        if (!\is_string($value) || \preg_match('/^([1-9][0-9]*)(m|h|d|w)$/', $value, $matches) !== 1) {
            throw new ParseError(
                "period: takes a duration such as '15m', '6h', '7d', '2w', 'all', or seconds.",
                $node->column(),
            );
        }

        $multiplier = match ($matches[2]) {
            'm' => 60,
            'h' => 3_600,
            'd' => 86_400,
            'w' => 604_800,
        };
        $seconds = (int) $matches[1] * $multiplier;

        if ($seconds > 31_536_000) {
            throw new ParseError(
                "period: is capped at 365d; use 'all' for the whole retention window.",
                $node->column(),
            );
        }

        return $seconds;
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
            'dropDuplicates' => $frame->dropDuplicates(...$this->references($step)),
            'rename' => $this->rename($frame, $step),
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

    private function rename(DataFrame $frame, Step $step): DataFrame
    {
        if (\count($step->args) !== 2) {
            throw new ParseError('rename() takes the old and new column names.', $step->column);
        }

        $from = $this->scalar($step->args[0]->value, 'rename source');
        $to = $this->scalar($step->args[1]->value, 'rename target');

        if (!\is_string($from) || !\is_string($to) || $to === '') {
            throw new ParseError('rename() takes two non-empty column-name strings.', $step->column);
        }
        if (!isset($this->known[$from])) {
            throw new ParseError(
                $this->dataset?->explainUnknownColumn($from) ?? \sprintf('Unknown column "%s".', $from),
            );
        }

        unset($this->known[$from]);
        $this->known[$to] = true;

        return $frame->rename($from, $to);
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
            'all' => all(...$this->scalarFns($node, $args)),
            'any' => any(...$this->scalarFns($node, $args)),
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

    /**
     * @param list<mixed> $args
     *
     * @return list<ScalarFunction>
     */
    private function scalarFns(Call $node, array $args): array
    {
        if ($args === []) {
            throw new ParseError(\sprintf('%s() needs at least one condition.', $node->name), $node->column());
        }

        foreach ($args as $argument) {
            if (!$argument instanceof ScalarFunction) {
                throw new ParseError(\sprintf('%s() takes conditions.', $node->name), $node->column());
            }
        }

        /** @var list<ScalarFunction> $args */
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
