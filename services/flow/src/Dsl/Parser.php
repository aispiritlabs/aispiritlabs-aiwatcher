<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dsl;

use Aiwatcher\Flow\Statistics\Descriptive;

/**
 * Reads a query into a tree of calls. Builds nothing and runs nothing.
 *
 * The grammar is small on purpose:
 *
 * ```text
 * query  := 'data_frame' '(' ')' step* ';'?
 * step   := '->' method '(' args? ')'
 * args   := arg (',' arg)* ','?
 * arg    := name ':' value | value
 * value  := call | string | number | bool | null | bareword
 * call   := name '(' args? ')' ('->' 'as' '(' string ')')?
 * ```
 *
 * `bareword` is how a dataset is named — `read(default)` — and the only place a
 * bare identifier is a value rather than a call.
 *
 * Separating this from [`PipelineBuilder`] is deliberate: parsing decides
 * whether the text is a legal query, building decides what it does. Keeping
 * them apart means the whole grammar can be tested without a network, a server
 * or any Flow object at all, which is what makes the rejection tests cheap
 * enough to be exhaustive.
 */
final class Parser
{
    private function __construct(
        private readonly Lexer $lexer,
    ) {}

    public static function parse(string $source): Query
    {
        $source = \trim($source);

        if ($source === '') {
            throw new ParseError('The query is empty.');
        }

        return (new self(new Lexer($source)))->query();
    }

    private function query(bool $nested = false): Query
    {
        $head = $this->expectName();

        if ($head->text !== 'data_frame' && $head->text !== 'df') {
            throw new ParseError('A query starts with data_frame().', $head->column, $head->text);
        }

        $this->expect('(');
        $this->expect(')');

        $steps = [];

        while (!$this->lexer->done()) {
            // A nested query is an argument, so it ends where the argument
            // list does: at the comma before the next one or at the bracket
            // that closes the call it sits in.
            if ($nested && ($this->lexer->peek()?->is(',') || $this->lexer->peek()?->is(')'))) {
                break;
            }

            if ($this->lexer->peek()?->is(';')) {
                $this->lexer->next();

                if (!$this->lexer->done()) {
                    $token = $this->lexer->peek();
                    throw new ParseError('A query is a single statement.', $token->column ?? 0, $token?->text);
                }

                break;
            }

            $this->expect('->');
            $name = $this->expectName();

            if (!Frame::isStep($name->text)) {
                throw new ParseError($this->explainStep($name->text), $name->column, $name->text);
            }

            $this->expect('(');
            $args = $this->arguments();
            $this->expect(')');

            $steps[] = new Step($name->text, $args, $name->column);
        }

        if ($steps === []) {
            throw new ParseError('The query reads nothing. Start with ->read(default).');
        }

        return new Query($steps);
    }

    /** @return list<Argument> */
    private function arguments(): array
    {
        $args = [];

        while (!$this->lexer->peek()?->is(')')) {
            if ($this->lexer->done()) {
                throw new ParseError('The query ends in the middle of an argument list.', $this->lexer->endColumn());
            }

            $name = null;

            // A named argument: `truncate: false`. Two tokens of lookahead
            // distinguish it from a bareword value.
            if ($this->lexer->peek()?->isName() && $this->lexer->peek(1)?->is(':')) {
                $name = $this->lexer->next()?->text;
                $this->lexer->next();
            }

            $args[] = new Argument($name, $this->value());

            if ($this->lexer->peek()?->is(',')) {
                $this->lexer->next();
            } elseif (!$this->lexer->peek()?->is(')')) {
                $token = $this->lexer->peek();
                throw new ParseError(
                    'Expected a comma or a closing bracket.',
                    $token->column ?? $this->lexer->endColumn(),
                    $token?->text,
                );
            }
        }

        return $args;
    }

    private function value(): Node
    {
        // A whole query, where a value goes: the right-hand side of a join.
        $ahead = $this->lexer->peek();

        if (
            $ahead !== null
            && ($ahead->text === 'data_frame' || $ahead->text === 'df')
            && $this->lexer->peek(1)?->is('(')
        ) {
            return new Nested($this->query(nested: true), $ahead->column);
        }

        $token = $this->lexer->next();

        if ($token === null) {
            throw new ParseError('The query ends where a value was expected.', $this->lexer->endColumn());
        }

        if ($token->id === \T_CONSTANT_ENCAPSED_STRING) {
            return new Literal(self::unquote($token->text), $token->column);
        }

        if ($token->id === \T_LNUMBER) {
            return new Literal((int) $token->text, $token->column);
        }

        if ($token->id === \T_DNUMBER) {
            return new Literal((float) $token->text, $token->column);
        }

        if ($token->is('-') && ($this->lexer->peek()?->id === \T_LNUMBER || $this->lexer->peek()?->id === \T_DNUMBER)) {
            $number = $this->lexer->next();

            return new Literal(
                $number?->id === \T_LNUMBER ? -(int) $number->text : -(float) ($number->text ?? '0'),
                $token->column,
            );
        }

        if (!$token->isName()) {
            throw new ParseError('Expected a value.', $token->column, $token->text);
        }

        $lowered = \strtolower($token->text);

        if (\in_array($lowered, ['true', 'false', 'null'], true)) {
            // A bareword `true`/`false`/`null` not followed by `(` is a literal;
            // otherwise it is someone calling a function by that name, which is
            // not in the whitelist and will be refused below.
            if (!$this->lexer->peek()?->is('(')) {
                return new Literal(match ($lowered) {
                    'true' => true,
                    'false' => false,
                    default => null,
                }, $token->column);
            }
        }

        // A bare name with no call brackets is a dataset: `read(default)`.
        if (!$this->lexer->peek()?->is('(')) {
            return new Bareword($token->text, $token->column);
        }

        if (!self::names($token->text)) {
            throw new ParseError($this->explainName($token->text), $token->column, $token->text);
        }

        $this->expect('(');
        $args = $this->arguments();
        $this->expect(')');

        $call = new Call($token->text, $args, $token->column);

        return $this->referenceMethods($call);
    }

    /**
     * `->as('name')`, `->asc()` and `->desc()` on a value.
     *
     * Flow puts all three on the *reference*, not on the surrounding call:
     * `count(ref('run_id')->as('runs'))`, `ref('duration_ms')->desc()`. Written
     * the other way round the alias is a fatal error deep inside Flow, so the
     * mistake is caught here with the correct form spelled out — it is the
     * first thing everyone gets wrong.
     */
    private function referenceMethods(Call $call): Node
    {
        while ($this->lexer->peek()?->is('->')) {
            $arrow = $this->lexer->next();
            $method = $this->expectName();

            $this->expect('(');
            $args = $this->arguments();
            $this->expect(')');

            if ($method->text === 'as') {
                if (\count($args) !== 1 || !$args[0]->value instanceof Literal || !\is_string($args[0]->value->value)) {
                    throw new ParseError('->as(...) takes one string.', $method->column);
                }

                $call = new Call(
                    $call->name,
                    $call->args,
                    $call->column(),
                    $args[0]->value->value,
                    $call->order,
                    $call->chain,
                );

                continue;
            }

            if (\in_array($method->text, ['asc', 'desc'], true)) {
                if ($args !== []) {
                    throw new ParseError(\sprintf('->%s() takes no arguments.', $method->text), $method->column);
                }

                $call = new Call($call->name, $call->args, $call->column(), $call->alias, $method->text, $call->chain);

                continue;
            }

            $call = new Call($call->name, $call->args, $call->column(), $call->alias, $call->order, [
                ...$call->chain,
                new Step($method->text, $args, $method->column),
            ]);
        }

        return $call;
    }

    /**
     * Whether a query may name this at all.
     *
     * Three sources and no fourth: everything Flow offers in an admitted
     * category ([`Registry`], by return type and by refusing any parameter
     * that accepts a callable), the three sinks that mean "give the rows
     * back", and the statistics this service implements because Flow ships
     * none. The first is derived from signatures; the other two are enums that
     * are their own implementation.
     */
    private static function names(string $name): bool
    {
        return Registry::has($name) || Sink::tryFrom($name) !== null || Descriptive::tryFrom($name) !== null;
    }

    /**
     * Why a name is not a step, which is a different question from the above.
     *
     * `write` is a step and not a value; `median` is a value and not a step.
     * Answering both with "not part of the query language" would be true and
     * useless — the second half of this message is the one somebody acts on.
     */
    private function explainStep(string $name): string
    {
        $declined = Frame::declined($name);

        if ($declined !== null) {
            return \sprintf('->%s() is deliberately not available. %s', $name, $declined);
        }

        if (self::names($name)) {
            return \sprintf(
                '"%s" is a value rather than a step: it goes inside one, e.g. ->withEntry(\'x\', %s(...)).',
                $name,
                $name,
            );
        }

        $nearest = self::nearest($name);

        return $nearest === null
            ? \sprintf('"%s" is not a pipeline step.', $name)
            : \sprintf('"%s" is not a pipeline step. Did you mean "%s"?', $name, $nearest);
    }

    private function explainName(string $name): string
    {
        $declined = Registry::declined($name);

        if ($declined !== null) {
            return \sprintf('"%s" is deliberately not available. %s', $name, $declined);
        }

        $nearest = self::nearest($name);

        return $nearest === null
            ? \sprintf('"%s" is not part of the query language.', $name)
            : \sprintf('"%s" is not part of the query language. Did you mean "%s"?', $name, $nearest);
    }

    /**
     * The nearest thing a query could have said, when there is one close enough.
     *
     * Over everything a query may write — the vocabulary, the steps and the
     * methods a value supports — because somebody who typed `groupby` and
     * somebody who mistyped `average` have made the same kind of mistake and
     * neither of them cares which list the right word came from.
     */
    private static function nearest(string $name): ?string
    {
        $closest = null;
        $distance = \PHP_INT_MAX;

        $everything = [
            ...Registry::names(),
            ...Frame::names(),
            ...Sink::names(),
            ...Descriptive::names(),
        ];

        foreach ($everything as $known) {
            $candidate = \levenshtein($name, $known);

            if ($candidate >= $distance) {
                continue;
            }

            $distance = $candidate;
            $closest = $known;
        }

        return $distance <= 3 ? $closest : null;
    }

    private function expect(string $text): Token
    {
        $token = $this->lexer->next();

        if ($token === null) {
            throw new ParseError(\sprintf('The query ends where "%s" was expected.', $text), $this->lexer->endColumn());
        }

        if (!$token->is($text)) {
            throw new ParseError(\sprintf('Expected "%s".', $text), $token->column, $token->text);
        }

        return $token;
    }

    private function expectName(): Token
    {
        $token = $this->lexer->next();

        if ($token === null || !$token->isName()) {
            throw new ParseError('Expected a name.', $token->column ?? $this->lexer->endColumn(), $token?->text);
        }

        return $token;
    }

    /** Undo PHP's single- or double-quoted string escaping. */
    private static function unquote(string $text): string
    {
        $quote = $text[0];
        $inner = \substr($text, 1, -1);

        return $quote === "'" ? \str_replace(["\\'", '\\\\'], ["'", '\\'], $inner) : \stripcslashes($inner);
    }
}
