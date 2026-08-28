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
    ) {}
}
