<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

/**
 * A managed query somebody stopped: the run it belonged to was cancelled, or its step ran
 * past its deadline. Not a failure of the query, so it is answered as its own status.
 */
final class QueryCancelled extends \RuntimeException
{
    public function __construct(string $executionId)
    {
        parent::__construct(\sprintf('The query was cancelled: the run %s belonged to stopped.', $executionId));
    }
}
