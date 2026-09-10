<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

use Aiwatcher\Flow\Dataset\Dataset;
use Flow\ETL\DataFrame;

/**
 * A built, not-yet-executed query.
 *
 * The frame is deliberately not run here: the row cap and the timeout belong to
 * whoever serves the request, not to the language.
 */
final readonly class Plan
{
    public function __construct(
        public DataFrame $frame,
        public ?Dataset $dataset,
        /** From `to_output(truncate:)`. The panel shortens long cells unless told not to. */
        public bool $truncate,
        /** The effective relative period after the script overrides the panel, if it does. */
        public ?int $windowSeconds,
        /**
         * Whether running this query again would answer the same thing.
         *
         * False once a query names `now()`, `uuid_v4()` or another function
         * whose value is the moment it ran. Carried out to the caller rather
         * than acted on here: only a managed step cares, and it cares by
         * declining to remember the result.
         */
        public bool $deterministic = true,
    ) {}
}
