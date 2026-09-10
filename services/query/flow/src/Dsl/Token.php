<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/** One lexed token. `id` is null for single-character tokens like `(` or `,`. */
final readonly class Token
{
    public function __construct(
        public ?int $id,
        public string $text,
        public int $line,
        /** Offset into the query the caller sent, so an error can point at it. */
        public int $column,
    ) {}

    public function is(string $text): bool
    {
        return $this->text === $text;
    }

    public function isName(): bool
    {
        // `default` is a PHP keyword, so it arrives as T_DEFAULT rather than
        // T_STRING. It is also the name of a dataset and the most likely thing
        // anyone writes first, so it has to be accepted as a bare name.
        return $this->id === \T_STRING || $this->id === \T_DEFAULT;
    }
}
