<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

/**
 * Turns query text into tokens, using PHP's own lexer and running none of it.
 *
 * `token_get_all()` is the right primitive here: it is the same lexer the engine
 * uses, so quoting, escapes, numeric formats and named arguments are handled
 * exactly as PHP handles them — and it *only* lexes. Nothing is compiled and
 * nothing is executed. A hand-rolled tokeniser for this grammar would be a
 * second, worse implementation of string escaping, which is where such things
 * go wrong.
 *
 * Everything the grammar has no use for is rejected here rather than deeper in,
 * so the parser only ever sees tokens from a small, known set. That ordering
 * matters: a rejection list applied after parsing is a rejection list with holes
 * in it.
 */
final class Lexer
{
    /**
     * Token types that can never appear in a query.
     *
     * Not an exhaustive list of dangerous PHP — the parser accepts only what it
     * recognises, so the default is already rejection. These are named
     * explicitly so that the error says *why*, and so the intent is on the
     * record: no variables, no closures, no object construction, no static
     * dispatch, no includes, no shelling out, no escaping to inline HTML.
     */
    private const array FORBIDDEN = [
        \T_VARIABLE => 'variables are not part of the query language',
        \T_FN => 'closures cannot be used here',
        \T_FUNCTION => 'closures cannot be used here',
        \T_NEW => 'objects cannot be constructed here',
        \T_DOUBLE_COLON => 'static calls are not part of the query language',
        \T_INCLUDE => 'includes are not part of the query language',
        \T_INCLUDE_ONCE => 'includes are not part of the query language',
        \T_REQUIRE => 'includes are not part of the query language',
        \T_REQUIRE_ONCE => 'includes are not part of the query language',
        \T_EVAL => 'eval is not part of the query language',
        \T_INLINE_HTML => 'the query must not close its PHP tag',
        \T_OPEN_TAG => 'the query must not open a PHP tag',
        \T_OPEN_TAG_WITH_ECHO => 'the query must not open a PHP tag',
        \T_CLOSE_TAG => 'the query must not close its PHP tag',
        \T_ECHO => 'echo is not part of the query language',
        \T_PRINT => 'print is not part of the query language',
        \T_NS_SEPARATOR => 'namespaced names are not part of the query language',
        // A double-quoted string with an interpolation lexes into these; only
        // T_CONSTANT_ENCAPSED_STRING (no interpolation) is accepted as a value.
        \T_ENCAPSED_AND_WHITESPACE => 'string interpolation is not supported; use single quotes',
        \T_DOLLAR_OPEN_CURLY_BRACES => 'string interpolation is not supported; use single quotes',
        \T_CURLY_OPEN => 'string interpolation is not supported; use single quotes',
    ];

    /** Single characters that can never appear. Backtick is shell execution. */
    private const array FORBIDDEN_CHARS = [
        '`' => 'backticks execute shell commands and are never allowed',
        '$' => 'variables are not part of the query language',
        '"' => 'use single-quoted strings',
        ';' => null, // handled separately: a single trailing one is fine
    ];

    /** @var list<Token> */
    private array $tokens;

    private int $position = 0;

    public function __construct(string $source)
    {
        $this->tokens = self::lex($source);
    }

    /** @return list<Token> */
    private static function lex(string $source): array
    {
        // The open tag is ours, not the caller's: a query is an expression, not
        // a PHP file. A caller that writes its own tag is rejected above.
        //
        // Deliberately without TOKEN_PARSE. That flag adds PHP's *parser* on top
        // of the lexer, which rejects `read(default)` — PHP sees `default` as a
        // switch label needing a colon. Plain lexing is what is wanted anyway:
        // this grammar is not PHP's, and validating it is this file's job.
        $raw = \token_get_all('<?php ' . $source);

        $tokens = [];
        $offset = 0;
        $seenOpenTag = false;

        foreach ($raw as $token) {
            if (\is_array($token)) {
                [$id, $text, $line] = $token;

                if ($id === \T_OPEN_TAG && !$seenOpenTag) {
                    $seenOpenTag = true;
                    $offset += \strlen($text);

                    continue;
                }

                if ($id === \T_WHITESPACE || $id === \T_COMMENT || $id === \T_DOC_COMMENT) {
                    // Comments are stripped, not rejected: they cannot execute,
                    // and refusing them would be theatre rather than a boundary.
                    $offset += \strlen($text);

                    continue;
                }

                if (isset(self::FORBIDDEN[$id])) {
                    throw new ParseError(self::FORBIDDEN[$id], self::column($offset), $text);
                }

                $tokens[] = new Token($id, $text, $line, self::column($offset));
                $offset += \strlen($text);

                continue;
            }

            if (\array_key_exists($token, self::FORBIDDEN_CHARS) && self::FORBIDDEN_CHARS[$token] !== null) {
                throw new ParseError(self::FORBIDDEN_CHARS[$token], self::column($offset), $token);
            }

            $tokens[] = new Token(null, $token, 0, self::column($offset));
            $offset += \strlen($token);
        }

        return $tokens;
    }

    /** Offsets are measured against the source the caller sent, not our tag. */
    private static function column(int $offset): int
    {
        return \max(0, $offset - \strlen('<?php '));
    }

    public function peek(int $ahead = 0): ?Token
    {
        return $this->tokens[$this->position + $ahead] ?? null;
    }

    public function next(): ?Token
    {
        return $this->tokens[$this->position++] ?? null;
    }

    public function done(): bool
    {
        return $this->position >= \count($this->tokens);
    }

    /** The position a "unexpected end of query" error should point at. */
    public function endColumn(): int
    {
        $last = $this->tokens[\count($this->tokens) - 1] ?? null;

        return $last === null ? 0 : $last->column + \strlen($last->text);
    }
}
