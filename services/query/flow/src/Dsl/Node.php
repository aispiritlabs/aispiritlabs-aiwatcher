<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * A value in a query: a literal, a bare dataset name, or a whitelisted call.
 *
 * Everything carries its column so a rejection can point at the character
 * rather than at the query.
 */
interface Node
{
    public function column(): int;
}
