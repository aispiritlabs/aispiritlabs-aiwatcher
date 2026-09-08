<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

/**
 * aiwatcher answered, and the answer was not rows.
 *
 * Without this the failure arrived as `Path "rows" does not exists in array
 * ...` — Flow reaching into an error body for the key a successful response
 * would have had. The message a person needed was in there, spelled without its
 * spaces, inside a sentence about an array path.
 *
 * It carries the upstream status because the caller's next decision depends on
 * it: a 501 naming an unset variable will still be a 501 in ten minutes, and a
 * managed step that reads it as "nobody answered" spends its whole retry budget
 * finding that out.
 */
final class UpstreamFailed extends \RuntimeException
{
    public function __construct(
        public readonly int $status,
        public readonly string $detail,
        public readonly string $uri,
    ) {
        parent::__construct(\sprintf('aiwatcher answered %d for %s: %s', $status, $uri, $detail));
    }

    /**
     * Whether asking again could ever produce a different answer.
     *
     * A 4xx is this service asking wrongly and a 501 is the instance not doing
     * that at all — both are answers, and neither changes because a reactor
     * waited thirty seconds. Everything else is treated as worth another go,
     * which is the safe direction: retrying something permanent costs attempts,
     * and refusing to retry something transient costs the run.
     */
    public function isPermanent(): bool
    {
        return $this->status >= 400 && $this->status < 500 || $this->status === 501;
    }
}
