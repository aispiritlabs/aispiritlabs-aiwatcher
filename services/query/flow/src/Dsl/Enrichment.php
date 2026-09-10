<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

use Aiwatcher\Flow\Dataset\Catalog;

/**
 * Rewrites a query into something a PHP parser will accept.
 *
 * The query language is deliberately not PHP in one place: a dataset is named
 * bare, `->read(default)`. `default` is a PHP keyword, so a real PHP parser
 * stops on the first line of the canonical example — Mago reports
 * *"Unexpected token `Default`"*, and PHP's own `token_get_all(…, TOKEN_PARSE)`
 * refuses it as a switch label missing its colon.
 *
 * Enrichment substitutes the value the bareword stands for — `default` becomes
 * `'runs'` — which is enough to make the text parse as PHP without changing
 * what it means. That is what lets a general-purpose PHP linter say something
 * useful about a query.
 *
 * ## Offsets
 *
 * The rewrite changes lengths, and a diagnostic that points at the wrong
 * character is worse than no diagnostic. Every substitution is recorded, so
 * [`self::originalOffset`] maps a position in the enriched text back to the
 * position in what the person actually typed.
 *
 * Enrichment is **not** a security boundary. Nothing here decides what may run;
 * that is [`Parser`] and [`PipelineBuilder`]. This exists so a linter can read
 * the query, and the linter never executes it either.
 */
final readonly class Enrichment
{
    /** Prepended so the text is a complete PHP file, and so `strict-types` stops being reported. */
    public const string PREAMBLE = "<?php declare(strict_types=1);\n";

    /**
     * @param list<array{at: int, from: int, to: int}> $edits original offset, original length, replacement length
     */
    private function __construct(
        public string $php,
        private array $edits,
        /** Length of what the caller actually typed, so a diagnostic cannot point past it. */
        private int $originalLength,
    ) {}

    public static function apply(string $query, Catalog $catalog): self
    {
        $tokens = \token_get_all('<?php ' . $query);
        $tagLength = \strlen('<?php ');

        $offset = 0;
        $edits = [];
        $out = '';
        $previous = null;

        foreach ($tokens as $index => $token) {
            $text = \is_array($token) ? $token[1] : $token;
            $id = \is_array($token) ? $token[0] : null;
            $start = $offset - $tagLength;
            $offset += \strlen($text);

            if ($id === \T_OPEN_TAG) {
                continue;
            }

            $replacement = self::substitute($id, $text, $tokens, $index, $previous, $catalog);

            if ($replacement !== null) {
                $edits[] = ['at' => $start, 'from' => \strlen($text), 'to' => \strlen($replacement)];
                $out .= $replacement;
            } else {
                $out .= $text;
            }

            if ($id !== \T_WHITESPACE) {
                $previous = $text;
            }
        }

        // A query may end without a semicolon — the grammar treats it as
        // optional — but a PHP file may not, and Mago would report the missing
        // one as the only problem with an otherwise correct query. Appending it
        // shifts nothing, because it goes on the end.
        $terminated = \rtrim($out);

        if ($terminated !== '' && !\str_ends_with($terminated, ';')) {
            $out = $terminated . ';';
        }

        return new self(self::PREAMBLE . $out, $edits, \strlen($query));
    }

    /**
     * What a token becomes, or null to keep it as it is.
     *
     * Only a bare name that (a) directly follows `read(` and (b) names a
     * dataset is rewritten. Narrow on purpose: a broader rule would start
     * rewriting things inside a query that only look like dataset names, and
     * the enriched text has to keep meaning what the original meant.
     *
     * @param array<int, array{0: int, 1: string, 2: int}|string> $tokens
     */
    private static function substitute(
        ?int $id,
        string $text,
        array $tokens,
        int $index,
        ?string $previous,
        Catalog $catalog,
    ): ?string {
        if ($id !== \T_STRING && $id !== \T_DEFAULT) {
            return null;
        }

        if ($previous !== '(') {
            return null;
        }

        // Only inside `read(`. Two tokens back, skipping the bracket.
        if (self::nameBefore($tokens, $index) !== 'read') {
            return null;
        }

        // A name followed by `(` is a call, not a dataset.
        if (self::nextSignificant($tokens, $index) === '(') {
            return null;
        }

        $dataset = $catalog->resolve($text);

        // An unknown dataset is left alone: the parser reports it with the list
        // of real ones, which is a better message than anything a linter would
        // produce from a rewritten name.
        return $dataset === null ? null : "'" . $dataset->name . "'";
    }

    /** @param array<int, array{0: int, 1: string, 2: int}|string> $tokens */
    private static function nameBefore(array $tokens, int $index): ?string
    {
        $seenBracket = false;

        for ($cursor = $index - 1; $cursor >= 0; $cursor--) {
            $token = $tokens[$cursor];
            $text = \is_array($token) ? $token[1] : $token;

            if (\is_array($token) && $token[0] === \T_WHITESPACE) {
                continue;
            }

            if (!$seenBracket) {
                if ($text !== '(') {
                    return null;
                }

                $seenBracket = true;

                continue;
            }

            return $text;
        }

        return null;
    }

    /** @param array<int, array{0: int, 1: string, 2: int}|string> $tokens */
    private static function nextSignificant(array $tokens, int $index): ?string
    {
        for ($cursor = $index + 1, $count = \count($tokens); $cursor < $count; $cursor++) {
            $token = $tokens[$cursor];

            if (\is_array($token) && $token[0] === \T_WHITESPACE) {
                continue;
            }

            return \is_array($token) ? $token[1] : $token;
        }

        return null;
    }

    /**
     * Where a position in the enriched text sits in the original.
     *
     * Walks the substitutions in order, undoing each length change that happened
     * before the position. Clamped at zero so a diagnostic pointing into the
     * preamble lands on the first character rather than a negative one.
     */
    public function originalOffset(int $enriched): int
    {
        $offset = $enriched - \strlen(self::PREAMBLE);
        $shift = 0;

        foreach ($this->edits as $edit) {
            $replacementEnd = $edit['at'] + $shift + $edit['to'];

            if ($offset < $replacementEnd) {
                // Inside or before this substitution: point at where it started.
                return \max(0, \min($offset - $shift, $edit['at']));
            }

            $shift += $edit['to'] - $edit['from'];
        }

        // Clamped to the original text: a diagnostic about the semicolon this
        // added should land on the last character someone typed, not past it.
        return \max(0, \min($offset - $shift, \max(0, $this->originalLength - 1)));
    }
}
