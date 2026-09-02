<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

/**
 * One named argument a `read()` may carry into the API's query string.
 *
 * Declared rather than accepted, for the same reason the column list is
 * declared: the aiwatcher API **rejects** unknown query parameters instead of
 * ignoring them, so a parameter that exists here and not there turns a whole
 * query into a 400 with no clue which word caused it. What `/flow/datasets`
 * lists is exactly what a query may write.
 *
 * `run:` and `period:` are deliberately not modelled here. `run:` substitutes
 * a path segment rather than a query parameter, and `period:` is the panel's
 * time control, which every windowed dataset shares and no dataset declares.
 */
final readonly class Parameter
{
    public function __construct(
        public string $name,
        /**
         * Whether the route is unusable without it.
         *
         * A required parameter fails at parse time with a sentence saying what
         * to write, rather than at request time with somebody else's 400.
         */
        public bool $required,
        public string $description,
        /**
         * The values this may take, when it is a closed set.
         *
         * Checked here so `hub: 'huggingfaec'` is a parse error naming the two
         * that exist, instead of an empty result that reads as "no matches".
         */
        public array $values = [],
    ) {}

    public function accepts(string $value): bool
    {
        return $this->values === [] || \in_array($value, $this->values, true);
    }
}
