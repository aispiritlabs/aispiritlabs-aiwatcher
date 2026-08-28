<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * A query that was refused.
 *
 * Carries the offset so the panel can point at the character rather than
 * saying "invalid query" and leaving someone to guess. Every rejection is one
 * of these — a rejected query never becomes a partially-built pipeline.
 */
final class ParseError extends \RuntimeException
{
    public function __construct(
        string $message,
        public readonly int $column = 0,
        public readonly ?string $near = null,
    ) {
        parent::__construct($near === null ? $message : \sprintf('%s (near "%s")', $message, $near));
    }

    /** @return array{message: string, column: int, near: string|null} */
    public function toArray(): array
    {
        return ['message' => $this->getMessage(), 'column' => $this->column, 'near' => $this->near];
    }
}
