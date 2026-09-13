<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

use Aiwatcher\Flow\Dataset\UpstreamFailed;
use Aiwatcher\Flow\Dsl\ParseError;

/**
 * How a request that did not answer is answered: one status and one error body.
 *
 * Decided once, because two places raise what it reads — the request itself, and the process
 * a managed query runs in, whose failure the request relays as it was decided there
 * (`QueryFailed`). A status worked out twice is a 422 in one and a 502 in the other for the
 * same query, and a reactor retries one of those ten times.
 */
final readonly class Failure
{
    /** @param array<string, mixed> $error */
    public function __construct(
        public int $status,
        public array $error,
    ) {}

    public static function of(\Throwable $error): self
    {
        return match (true) {
            // Its own status: a query somebody stopped is neither the query's fault nor an outage.
            $error instanceof QueryCancelled => new self(409, ['message' => $error->getMessage(), 'column' => 0]),
            // 422, not 400: the request was well-formed, the query was not. The column is what
            // lets the panel point at the character instead of the query.
            $error instanceof ParseError => new self(422, $error->toArray()),
            // aiwatcher answered, and its answer decides this one. A 501 naming an unset
            // variable is relayed as a 4xx so the caller reads it as permanent: a managed step
            // classifies a 5xx from here as "nobody answered" and spends ten attempts over ten
            // minutes discovering that a configuration flag is still off. Everything else
            // stays 502, which is the honest reading of a store that may come back.
            $error instanceof UpstreamFailed => new self($error->isPermanent() ? 422 : 502, [
                'message' => \sprintf('The query could not be run: %s', $error->getMessage()),
                'column' => 0,
            ]),
            // Decided where it was raised, in the process the query ran in.
            $error instanceof QueryFailed => new self($error->status, $error->error),
            // Anything else is aiwatcher being unreachable, or a bug here. Both are worth
            // saying plainly rather than as an empty table.
            default => new self(502, [
                'message' => \sprintf('The query could not be run: %s', $error->getMessage()),
                'column' => 0,
            ]),
        };
    }
}
