<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dataset\Dataset;
use Aiwatcher\Flow\Statistics\Descriptive;
use Aiwatcher\Flow\Statistics\Statistic;
use Flow\ETL\DataFrame;
use Flow\ETL\DataFrame\GroupedDataFrame;
use Flow\ETL\Function\AggregatingFunction;
use Flow\ETL\Function\ScalarFunction;
use Flow\ETL\Function\WindowFunction;
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
 * A name from the query is only ever resolved against something looked up
 * first: an arm of a `match` here, or a `ReflectionMethod` that [`Frame`] or
 * [`Values`] admitted by signature. `$name(...)` — a *global* call by string —
 * appears nowhere, and that is the difference that matters: the reachable set
 * is fixed and derived from types, and every member of it reshapes rows rather
 * than reaching a file, a socket or a callable.
 *
 * The arms below are the steps that need something this file knows and a
 * signature does not — the catalog, the window, which columns exist afterwards.
 * Everything else Flow's `DataFrame` offers goes through [`Frame`], which is
 * why `join()` works without anybody adding an arm for it.
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

    /** @var array<string, true> What a nested query brought, until the join takes it. */
    private array $joined = [];

    private bool $truncate = true;

    /**
     * Whether every function this query named answers the same thing twice.
     *
     * Reported rather than refused: `now()` and `uuid_v4()` are legitimate in
     * an ad-hoc query, which is never cached anyway. What they may not do is
     * let a managed step be remembered as a pure function of its inputs.
     */
    private bool $deterministic = true;

    private ?Dataset $dataset = null;

    /** The request window, overridden by read(..., period:) when the script pins one. */
    private ?int $effectiveWindowSeconds;

    public function __construct(
        private readonly Catalog $catalog,
        /** The panel's fallback time window, in seconds, or null for everything. */
        ?int $windowSeconds = null,
        /**
         * The instant a managed plan pinned its window to end at.
         *
         * Absent for every panel query, which is what keeps a shared link
         * meaning "the last hour" when it is opened. Present, the window is a
         * closed span and a retry reads the same rows — which is what lets a
         * managed step be cached at all.
         *
         * Not overridden by `period:` the way the width is: a script that pins
         * its own period is saying how *wide*, and the plan is saying how far
         * *back from where*. The two compose.
         */
        private readonly ?int $asOf = null,
        /** Bound hub input during simulation, including nested reads. */
        private readonly ?int $inputLimit = null,
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

        return new Plan($frame, $this->dataset, $this->truncate, $this->effectiveWindowSeconds, $this->deterministic);
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

        // Files on disk are addressed by nothing: the same query over the same
        // name reads whatever the directory holds today, so a remembered answer
        // would be a claim about bytes that may since have been regenerated.
        // Said here, where `now()` says it, for the reactor's `cacheable`.
        if ($dataset->corpus !== null) {
            $this->deterministic = false;
        }

        return $this->catalog->open(
            $dataset,
            \is_string($run) ? $run : null,
            $this->effectiveWindowSeconds,
            $arguments,
            $this->asOf,
            $this->inputLimit,
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
            'trainTestSplit', 'imputeMissing', 'oneHotEncode', 'labelEncode' => $this->flowAI($frame, $step),
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
            // Everything else Flow offers under the admission rule: join,
            // crossJoin, offset, until, batchBy, collect, void… Nothing here
            // had to be written down for those to work, which is the point.
            default => $this->step($frame, $step),
        };
    }

    /**
     * Whether an aggregation was given a window to answer over.
     *
     * `WindowFunction::window()` throws when there is no OVER clause, which is
     * Flow's way of saying "not windowed" and the only way to ask.
     */
    private static function isWindowed(mixed $value): bool
    {
        if (!$value instanceof WindowFunction) {
            return false;
        }

        try {
            $value->window();

            return true;
        } catch (\Throwable) {
            return false;
        }
    }

    /**
     * A step this file has no arm for, applied through the one Flow declares.
     *
     * This is where `join`, `crossJoin`, `offset`, `until`, `batchBy`,
     * `collect` and the rest arrived from — not by being written down, but by
     * [`Frame`] admitting every `DataFrame` method that returns a frame and
     * takes nothing [`Admission`] refuses. A step that was refused says which
     * of those two rules refused it.
     */
    private function step(DataFrame $frame, Step $step): DataFrame
    {
        $method = Frame::method($step->name);

        if ($method === null) {
            throw new ParseError(\sprintf('"%s" is not a pipeline step.', $step->name), $step->column);
        }

        $before = $this->known;
        $arguments = $this->bind($method, $step->name, $step->args, $step->column);
        $result = $this->invoke(
            static fn(): mixed => $method->invokeArgs($frame, $arguments),
            $step->name,
            $method,
            $step->column,
        );

        if (!$result instanceof DataFrame) {
            throw new ParseError(\sprintf('->%s() did not produce a frame.', $step->name), $step->column);
        }

        // A join is the one admitted step that changes which columns exist, so
        // it is the one this has to say something about. Everything else Flow
        // admits here reshapes rows rather than columns, and leaving the set
        // alone keeps "unknown column" answering the question it answers.
        if (\in_array($step->name, ['join', 'crossJoin'], true)) {
            $this->known = [...$before, ...$this->joined];
        }

        $this->joined = [];

        return $result;
    }

    private function flowAI(DataFrame $frame, Step $step): DataFrame
    {
        $allowed = match ($step->name) {
            'trainTestSplit' => ['target', 'output', 'fraction', 'seed'],
            'imputeMissing' => ['output', 'strategy', 'groupBy', 'fitOn', 'fitValue', 'missingIndicator'],
            'oneHotEncode' => ['output', 'fitOn', 'fitValue', 'handleUnknown', 'state', 'stateOutput'],
            'labelEncode' => ['output', 'fitOn', 'fitValue', 'state', 'stateOutput'],
        };
        $columns = [];
        $options = [];
        foreach ($step->args as $argument) {
            $value = $this->scalar($argument->value, $step->name);
            if ($argument->name === null) {
                if ($options !== [] || !\is_string($value) || $value === '') {
                    throw new ParseError('FlowAI expects column names before named options.', $step->column);
                }
                $columns[] = $value;
            } else {
                if (!\in_array($argument->name, $allowed, true) || \array_key_exists($argument->name, $options)) {
                    throw new ParseError('Unknown or duplicate FlowAI option: ' . $argument->name, $step->column);
                }
                if ($argument->name === 'fraction') {
                    if (!\is_int($value) && !\is_float($value) || $value <= 0 || $value >= 1) {
                        throw new ParseError('fraction must be a number between 0 and 1.', $step->column);
                    }
                } elseif ($argument->name === 'seed') {
                    if (!\is_int($value)) {
                        throw new ParseError('seed must be an integer.', $step->column);
                    }
                } elseif ($argument->name !== 'fitValue' && (!\is_string($value) || $value === '')) {
                    throw new ParseError($argument->name . ' must be a nonempty string.', $step->column);
                }
                $options[$argument->name] = $value;
            }
        }
        if (
            $columns === []
            || \count(\array_unique($columns)) !== \count($columns)
            || $step->name !== 'oneHotEncode' && \count($columns) !== 1
        ) {
            throw new ParseError('FlowAI requires unique input columns (one except for oneHotEncode).', $step->column);
        }
        if ($step->name === 'trainTestSplit' && !isset($options['target'])) {
            throw new ParseError('trainTestSplit requires target.', $step->column);
        }
        if (isset($options['strategy']) && !\in_array($options['strategy'], ['median', 'most_frequent'], true)) {
            throw new ParseError('strategy must be median or most_frequent.', $step->column);
        }
        if (isset($options['handleUnknown']) && !\in_array($options['handleUnknown'], ['ignore', 'error'], true)) {
            throw new ParseError('handleUnknown must be ignore or error.', $step->column);
        }
        $references = [...$columns];
        foreach (['fitOn', 'target'] as $option) {
            if (isset($options[$option]) && !($option === 'fitOn' && isset($options['state']))) {
                $references[] = $options[$option];
            }
        }
        if (isset($options['groupBy'])) {
            $references = [...$references, ...\array_map('trim', \explode(',', $options['groupBy']))];
        }
        foreach ($references as $column) {
            if (!isset($this->known[$column])) {
                throw new ParseError('Unknown FlowAI input column: ' . $column, $step->column);
            }
        }
        $options['output'] ??= match ($step->name) {
            'trainTestSplit' => '_split',
            'oneHotEncode' => 'model_features',
            'labelEncode' => 'target_encoded',
            'imputeMissing' => $columns[0] . 'Filled',
        };
        $outputs = [$options['output']];
        foreach (['stateOutput', 'missingIndicator'] as $option) {
            if (isset($options[$option])) {
                if (\in_array($options[$option], $references, true)) {
                    throw new ParseError($option . ' must not overwrite an input column.', $step->column);
                }
                $outputs[] = $options[$option];
            }
        }
        if (\count(\array_unique($outputs)) !== \count($outputs)) {
            throw new ParseError('FlowAI output columns must be distinct.', $step->column);
        }
        foreach ($outputs as $column) {
            $this->known[$column] = true;
        }
        return (new \Aiwatcher\Flow\FlowAI\Preparation($step->name, $columns, $options))->apply($frame);
    }

    /**
     * The arguments of a call, in the order the signature wants them.
     *
     * Named arguments are matched by parameter name — `type: 'left'` — which
     * is how somebody writes a join without remembering that the expression
     * comes second. A variadic parameter takes everything that is left.
     *
     * @param list<Argument> $args
     *
     * @return list<mixed>
     */
    private function bind(\ReflectionFunctionAbstract $method, string $name, array $args, int $column): array
    {
        $parameters = $method->getParameters();
        $positional = [];
        $named = [];

        foreach ($args as $index => $argument) {
            $value = $this->argument($argument->value);

            if ($argument->name === null) {
                $positional[] = self::coerce($value, $parameters[$index] ?? null);

                continue;
            }

            $named[$argument->name] = $value;
        }

        $bound = $positional;

        foreach ($parameters as $index => $parameter) {
            if (!\array_key_exists($parameter->getName(), $named)) {
                continue;
            }

            if ($index < \count($bound)) {
                throw new ParseError(\sprintf('->%s() was given %s twice.', $name, $parameter->getName()), $column);
            }

            // Positions between the last positional argument and this one are
            // filled with their own defaults, which is what PHP would do.
            while (\count($bound) < $index) {
                $missing = $parameters[\count($bound)];

                if (!$missing->isOptional()) {
                    throw new ParseError(
                        \sprintf(
                            '->%s() needs %s. It is written %s.',
                            $name,
                            $missing->getName(),
                            Admission::signature($name, $method),
                        ),
                        $column,
                    );
                }

                $bound[] = $missing->getDefaultValue();
            }

            $bound[] = self::coerce($named[$parameter->getName()], $parameter);
            unset($named[$parameter->getName()]);
        }

        if ($named !== []) {
            throw new ParseError(
                \sprintf(
                    '->%s() has no argument called %s. It is written %s.',
                    $name,
                    \implode(', ', \array_keys($named)),
                    Admission::signature($name, $method),
                ),
                $column,
            );
        }

        return $bound;
    }

    /**
     * A written value, as the parameter it is going into needs it.
     *
     * One coercion, and it is here because of a hole the admission rule cannot
     * close on its own: some of Flow's parameters take a **pure enum** —
     * `Rounding` on `divide()` is the one that bites — and a query has no way
     * to write one, because `::` is not part of this language and never will
     * be. Without this, division throws from inside Brick\Math the moment a
     * result does not divide exactly, which is most of the time.
     *
     * Matched on the case *name*, case-insensitively, so
     * `->divide(ref('n'), lit(2), 'half_up')` reads like the rest of the
     * language. Derived rather than listed: it is the parameter's own declared
     * type that says which enum, so an enum Flow adds later works the same
     * way.
     */
    private static function coerce(mixed $value, ?\ReflectionParameter $parameter): mixed
    {
        if (!\is_string($value) || $parameter === null) {
            return $value;
        }

        foreach (self::enums($parameter) as $enum) {
            foreach ($enum->getCases() as $case) {
                if (\strcasecmp($case->getName(), $value) === 0) {
                    return $case->getValue();
                }
            }
        }

        return $value;
    }

    /**
     * The enum types a parameter accepts, if any.
     *
     * @return list<\ReflectionEnum<\UnitEnum>>
     */
    private static function enums(\ReflectionParameter $parameter): array
    {
        $type = $parameter->getType();
        $types = $type instanceof \ReflectionUnionType ? $type->getTypes() : [$type];
        $enums = [];

        foreach ($types as $one) {
            if (!$one instanceof \ReflectionNamedType || $one->isBuiltin() || !\enum_exists($one->getName())) {
                continue;
            }

            $enums[] = new \ReflectionEnum($one->getName());
        }

        return $enums;
    }

    /** One argument, which may itself be a whole query. */
    private function argument(Node $node): mixed
    {
        if ($node instanceof Nested) {
            return $this->nested($node);
        }

        return $this->node($node);
    }

    /**
     * The right-hand side of a join: another query, read the same way.
     *
     * A child builder rather than this one, because the nested query has its
     * own dataset, its own columns and its own steps — and because the two
     * `known` sets have to stay apart until the join puts them together. It
     * shares the catalog, the window and the pinned instant, so a join never
     * reads a different span from the query it is joined into.
     */
    private function nested(Nested $node): DataFrame
    {
        $builder = new self($this->catalog, $this->effectiveWindowSeconds, $this->asOf, $this->inputLimit);
        $plan = $builder->build($node->query);

        // What the right side brings, for the column check after the join.
        $this->joined = $builder->known;

        if (!$plan->deterministic) {
            $this->deterministic = false;
        }

        return $plan->frame;
    }

    /**
     * Call it, and turn Flow's type errors into the message a person needs.
     *
     * A signature is checked by PHP rather than re-implemented here, so a
     * wrong argument arrives as a `TypeError` naming a parameter nobody wrote.
     * What is useful instead is how the call is written.
     *
     * @param \Closure(): mixed $call
     */
    private function invoke(\Closure $call, string $name, \ReflectionFunctionAbstract $method, int $column): mixed
    {
        try {
            return $call();
        } catch (\TypeError|\ArgumentCountError $error) {
            throw new ParseError(\sprintf('%s is written %s.', $name, Admission::signature($name, $method)), $column);
        }
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
        $written = $step->args[1]->value;

        // An aggregation answers one row per group, so on its own it cannot
        // fill a column. With an OVER clause it can — that is exactly what a
        // window is for — so the refusal is only for the bare one, and it says
        // both ways out rather than only the one that changes the shape of the
        // result.
        if ($value instanceof AggregatingFunction && !self::isWindowed($value)) {
            throw new ParseError(
                \sprintf(
                    '%s() is an aggregation: it answers one row per group. Put it in aggregate() after groupBy(), '
                    . 'or give it a window — %s(...)->over(window()->partitionBy(ref(\'…\'))) answers it beside every row.',
                    $written instanceof Call ? $written->name : 'That',
                    $written instanceof Call ? $written->name : 'it',
                ),
                $written->column(),
            );
        }

        // `DataFrame::withEntry` takes either, and the second one is what a
        // windowed statistic is.
        if (!$value instanceof ScalarFunction && !$value instanceof WindowFunction) {
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

            if (!$call instanceof Call) {
                throw new ParseError(
                    'aggregate() takes aggregations, e.g. count(ref(\'run_id\')->as(\'runs\')).',
                    $argument->value->column(),
                );
            }

            $aggregation = $this->node($call);

            // What may sit here is decided by what the name turned out to
            // build, not by a list of names kept beside the ones that build
            // them. `count()` is an aggregation because `Count` implements
            // Flow's interface for one; `lower()` is not, and the message says
            // what it is instead rather than reciting what it is not.
            if (!$aggregation instanceof AggregatingFunction) {
                throw new ParseError(
                    \sprintf(
                        '%s() is not an aggregation: it answers a value per row, so it belongs in withEntry() or filter() rather than in aggregate().',
                        $call->name,
                    ),
                    $call->column(),
                );
            }

            $aggregations[] = $aggregation;
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

            if (!$sink instanceof Call || Sink::tryFrom($sink->name) === null) {
                throw new ParseError(
                    \sprintf('write() takes one of: %s.', \implode(', ', Sink::names())),
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
            // Ours rather than Flow's, and constructed here like everything
            // else: the name selects a branch, never a callable (ADR_0008).
            'median', 'stddev', 'variance' => new Statistic(
                $this->statisticOver($node, $args, 1),
                Descriptive::from($node->name),
            ),
            'percentile' => new Statistic(
                $this->statisticOver($node, $args, 2),
                Descriptive::Percentile,
                $this->percentage($node, $args),
            ),
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
            'optional' => optional($args[0]),
            'all' => all(...$this->scalarFns($node, $args)),
            'any' => any(...$this->scalarFns($node, $args)),
            // Sinks are handled by `write`; reaching here means one was used as
            // a value, which is not a thing.
            'to_output', 'to_array', 'to_memory' => throw new ParseError(
                \sprintf('%s() belongs inside write().', $node->name),
                $node->column(),
            ),
            default => $this->viaRegistry($node, $args),
        };

        // The first thing everyone gets wrong: Flow puts the alias on the
        // *reference*, so `count(ref('run_id'))->as('runs')` silently names
        // nothing. Answered here rather than in the parser because what makes
        // it wrong is what the name built, and only this side knows that.
        // `over()` is the legitimate thing to chain onto an aggregation — it is
        // how a group's answer is put beside the rows — so this is only about
        // the alias and the ordering, which belong on the reference.
        if ($value instanceof AggregatingFunction) {
            $chained = $node->alias !== null ? 'as' : $node->order;

            if ($chained !== null) {
                throw new ParseError(
                    \sprintf(
                        'Name the reference, not the aggregation: write %s(ref(\'…\')->%s(…)) rather than %s(…)->%s(…).',
                        $node->name,
                        $chained,
                        $node->name,
                        $chained,
                    ),
                    $node->column(),
                );
            }
        }

        // `ref`/`col` already applied their own chain above; applying it twice
        // would compare the comparison.
        return \in_array($node->name, ['ref', 'col'], true) ? $value : $this->methods($value, $node);
    }

    /**
     * Apply `->plus(...)`, `->same(...)`, `->over(...)` — whatever the value has.
     *
     * Flow puts an enormous fluent API on a reference and on every scalar
     * function: arithmetic, string work, dates, regular expressions, and
     * `over()` on an aggregation, which is how a group's answer is put back
     * beside the rows it was taken over. [`Values`] admits them the way
     * [`Registry`] admits functions — by signature, on the class of the object
     * actually in hand — so `ref('sib_sp')->plus(ref('parch'))` works and
     * `->equals(...)` is still refused with its reason.
     *
     * The list this replaced offered seventeen comparisons. It is the reason
     * this query language was said to have no arithmetic, which was never true
     * of the engine.
     */
    private function methods(mixed $value, Call $node): mixed
    {
        foreach ($node->chain as $method) {
            if (!\is_object($value)) {
                throw new ParseError(
                    \sprintf('->%s(...) needs a value, e.g. ref(\'…\')->%s(…).', $method->name, $method->name),
                    $method->column,
                );
            }

            $declined = Registry::declined($method->name);

            if ($declined !== null) {
                throw new ParseError(
                    \sprintf('->%s(...) is deliberately not available. %s', $method->name, $declined),
                    $method->column,
                );
            }

            $reflected = Values::method($value, $method->name);

            if ($reflected === null) {
                throw new ParseError($this->explainMethod($value, $method->name), $method->column);
            }

            $target = $value;
            $arguments = $this->bind($reflected, $method->name, $method->args, $method->column);
            $value = $this->invoke(
                static fn(): mixed => $reflected->invokeArgs($target, $arguments),
                $method->name,
                $reflected,
                $method->column,
            );
        }

        return $value;
    }

    /**
     * Why this value does not support that method.
     *
     * Named against the object in hand rather than against a list, because the
     * answer depends on it: an aggregation supports `over` and not `plus`, a
     * window supports `partitionBy` and neither.
     */
    private function explainMethod(object $value, string $name): string
    {
        $closest = null;
        $distance = \PHP_INT_MAX;

        foreach (Values::names($value) as $known) {
            $candidate = \levenshtein($name, $known);

            if ($candidate >= $distance) {
                continue;
            }

            $distance = $candidate;
            $closest = $known;
        }

        $what = \strrchr($value::class, '\\');
        $what = $what === false ? $value::class : \substr($what, 1);

        return $distance <= 3 && $closest !== null
            ? \sprintf('->%s(...) is not something a %s supports. Did you mean "%s"?', $name, $what, $closest)
            : \sprintf('->%s(...) is not something a %s supports.', $name, $what);
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
     * The one column a [`Statistic`] is taken over.
     *
     * A statistic reads exactly one column, so an extra argument is a mistake
     * worth naming rather than ignoring: `median(ref('age'), 90)` is somebody
     * reaching for `percentile`, and silently dropping the 90 would answer a
     * different question with no sign that it had.
     *
     * @param list<mixed> $args
     */
    private function statisticOver(Call $node, array $args, int $arity): Reference
    {
        if (\count($args) !== $arity) {
            throw new ParseError(
                $arity === 1
                    ? \sprintf('%s() takes one column, written as ref(\'name\').', $node->name)
                    : \sprintf(
                        '%s() takes a column and a percentage, e.g. %s(ref(\'age\'), 90).',
                        $node->name,
                        $node->name,
                    ),
                $node->column(),
            );
        }

        return $this->fns($node, \array_slice($args, 0, 1))[0];
    }

    /**
     * Which percentage `percentile()` was asked for.
     *
     * Bounded here rather than in the statistic, because this is the one place
     * that knows where the number came from — a query somebody typed, at a
     * column the message can point at.
     *
     * @param list<mixed> $args
     */
    private function percentage(Call $node, array $args): float
    {
        /** @var mixed $value */
        $value = $args[1] ?? null;

        if (!\is_int($value) && !\is_float($value)) {
            throw new ParseError(
                'percentile() takes a percentage as its second argument, e.g. percentile(ref(\'age\'), 90).',
                $node->column(),
            );
        }

        if ($value < 0 || $value > 100) {
            throw new ParseError(
                \sprintf('A percentile is between 0 and 100; %s is not.', (string) $value),
                $node->column(),
            );
        }

        return (float) $value;
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

        return $node->chain === [] ? $reference : $this->methods($reference, $node);
    }

    /**
     * A function Flow offers that this file has no bespoke arm for.
     *
     * The name is a **key** into [`Registry::functions()`] and never becomes a
     * callable, which is ADR 0008's rule stated where it actually bites: it is
     * about dispatch, not about enumeration. The arms above stay because they
     * marshal arguments in ways a signature does not describe — an aggregation
     * carries an alias from `->as()`, and `all()` takes scalar functions rather
     * than values.
     */
    private function viaRegistry(Call $node, array $args): mixed
    {
        $function = Registry::function($node->name);

        if ($function === null) {
            throw new ParseError(\sprintf('"%s" is not part of the query language.', $node->name), $node->column());
        }

        if (\in_array($node->name, Registry::NONDETERMINISTIC, true)) {
            $this->deterministic = false;
        }

        // Named arguments keep their names as string keys, which is what the
        // spread turns back into named arguments.
        $call = [];

        foreach ($node->args as $index => $argument) {
            if ($argument->name === null) {
                $call[] = $args[$index];

                continue;
            }

            $call[$argument->name] = $args[$index];
        }

        try {
            return $function->invoke(...$call);
        } catch (\TypeError|\ArgumentCountError) {
            // Flow's own message names a parameter position in a file nobody
            // reading a query has open. The signature is what the call was
            // checked against, so the signature is what the message says.
            throw new ParseError(
                \sprintf(
                    '%s() does not take those arguments. It is %s.',
                    $node->name,
                    Registry::signature($node->name),
                ),
                $node->column(),
            );
        }
    }
}
