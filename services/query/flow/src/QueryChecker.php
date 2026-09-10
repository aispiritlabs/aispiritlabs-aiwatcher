<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Lint\MagoLinter;

/**
 * What is wrong with a query, without running it.
 *
 * Two sources, because they know different things and neither subsumes the
 * other:
 *
 * * **Mago** is a real PHP parser. After enrichment it reads the query and
 *   reports where the brackets stopped making sense, with the precision of a
 *   compiler front end. It has no idea what a dataset or a column is.
 * * **The parser and builder** know the grammar, the whitelist and every
 *   dataset's columns. They are what decides whether a query may run, and they
 *   produce the messages that name the fix.
 *
 * Syntax first: a missing bracket makes everything after it meaningless, so
 * reporting "unknown column" on top of it would be noise. Only when the text
 * parses does the second pass have anything useful to say.
 *
 * Nothing here executes the query, and building a pipeline does not run it —
 * `PipelineBuilder` assembles a `DataFrame`, which stays cold until someone
 * fetches from it.
 */
final readonly class QueryChecker
{
    public function __construct(
        private Catalog $catalog,
        private MagoLinter $linter,
    ) {}

    /** @return array<string, mixed> */
    public function check(string $query): array
    {
        $syntax = $this->linter->check($query);

        if ($syntax !== []) {
            return [
                'ok' => false,
                'diagnostics' => $syntax,
                'checked_by' => ['mago'],
            ];
        }

        try {
            // Building touches the catalog and validates every column, and stops
            // there — no request is made and no rows are read.
            (new PipelineBuilder($this->catalog))->build(Parser::parse($query));
        } catch (ParseError $error) {
            return [
                'ok' => false,
                'diagnostics' => [[
                    'level' => 'error',
                    'message' => $error->getMessage(),
                    'offset' => $error->column,
                    'line' => 0,
                    'help' => null,
                ]],
                'checked_by' => $this->sources(),
            ];
        }

        return ['ok' => true, 'diagnostics' => [], 'checked_by' => $this->sources()];
    }

    /** @return list<string> */
    private function sources(): array
    {
        return $this->linter->available() ? ['mago', 'aiwatcher'] : ['aiwatcher'];
    }
}
