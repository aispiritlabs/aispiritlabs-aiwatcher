<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

/**
 * A managed query that failed in the process it ran in, carrying the answer decided there.
 *
 * The exception that was raised stayed in that process; what crossed the pipe is its status
 * and its body, which `Failure::of` relays unchanged.
 */
final class QueryFailed extends \RuntimeException
{
    /** @param array<string, mixed> $error */
    public function __construct(
        public readonly int $status,
        public readonly array $error,
    ) {
        $message = $error['message'] ?? null;

        parent::__construct(\is_string($message) ? $message : 'The query failed.');
    }
}
