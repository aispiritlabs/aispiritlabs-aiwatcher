<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * The three names `write()` accepts, and the only loaders a query may reach.
 *
 * [`Registry`] decides what a query may name, and it deliberately admits no
 * `Flow\ETL\Loader` at all: `write()` is where rows would leave this process,
 * and `to_csv('/etc/anything')` in a service with no authentication is a file
 * write whose name looks as harmless as any other. So the loaders are not a
 * hole punched in that rule — they are this enum, and it exists because all
 * three of them mean exactly one thing here: **give the rows back**.
 *
 * The enum is the implementation rather than a list beside one. `tryFrom()` is
 * the admission, the case is what `PipelineBuilder::write()` matches on, and a
 * fourth loader can only appear by somebody adding a case and deciding what it
 * would mean.
 *
 * `to_output(truncate: false)` carries the one real instruction any of them
 * has, which the panel honours: do not shorten the cells.
 */
enum Sink: string
{
    case Output = 'to_output';
    case Array_ = 'to_array';
    case Memory = 'to_memory';

    /** @return list<string> */
    public static function names(): array
    {
        return \array_map(static fn(self $sink): string => $sink->value, self::cases());
    }
}
