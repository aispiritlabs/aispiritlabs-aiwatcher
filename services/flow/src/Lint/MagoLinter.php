<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Lint;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Enrichment;

/**
 * Syntax diagnostics for a query, from Mago.
 *
 * Mago is a PHP parser, and a query is nearly PHP — near enough that once
 * [`Enrichment`] has substituted the bareword dataset names it parses cleanly,
 * and its parse errors are better than anything worth hand-writing:
 *
 * ```text
 * Expected one of `RightParenthesis`, found `Semicolon`   line 4
 * ```
 *
 * It parses; it never runs the query. Neither does anything else here.
 *
 * ## Where this sits
 *
 * Strictly an editor aid. Mago has no idea what a dataset, a column or the
 * whitelist are, so it cannot decide whether a query is *allowed* — that stays
 * with the parser and the builder, which are the security boundary. This runs
 * alongside them to catch the class of mistake they report tersely: a bracket
 * in the wrong place.
 *
 * ## When Mago is not installed
 *
 * `composer install --no-dev` leaves it out, and that is fine. Absence returns
 * no diagnostics rather than an error: the check endpoint still reports
 * everything the parser found.
 */
final readonly class MagoLinter
{
    /**
     * Codes worth showing for a query fragment.
     *
     * Mago's full rule set is aimed at a PHP codebase — naming, complexity,
     * missing declarations — none of which mean anything for five chained
     * calls. Only the syntax layer is kept; everything else would be noise
     * that teaches people to ignore the panel.
     */
    private const array KEPT = ['parse', 'syntax', 'unclosed', 'unexpected-token'];

    public function __construct(
        private Catalog $catalog,
        private string $binary,
        private int $timeoutSeconds = 5,
    ) {}

    public static function fromVendor(Catalog $catalog, string $root): self
    {
        return new self($catalog, self::locate($root) ?? '');
    }

    /**
     * The native binary, not the composer shim.
     *
     * `vendor/bin/mago` is a PHP script that locates and re-executes the real
     * binary, so calling it needs `php` on `PATH` — and this runs the linter
     * with a deliberately bare environment. Resolving the native executable
     * directly skips a process and the dependency.
     */
    private static function locate(string $root): ?string
    {
        $matches = \glob($root . '/vendor/carthage-software/mago/composer/bin/*/mago-*/mago');

        if ($matches === false) {
            $matches = [];
        }

        foreach ($matches as $candidate) {
            if (\is_executable($candidate)) {
                return $candidate;
            }
        }

        $shim = $root . '/vendor/bin/mago';

        return \is_executable($shim) ? $shim : null;
    }

    public function available(): bool
    {
        return $this->binary !== '' && \is_file($this->binary) && \is_executable($this->binary);
    }

    /**
     * @return list<array{level: string, message: string, offset: int, line: int, help: string|null}>
     */
    public function check(string $query): array
    {
        if (!$this->available() || \trim($query) === '') {
            return [];
        }

        $enriched = Enrichment::apply($query, $this->catalog);
        $workspace = $this->workspace($enriched->php);

        if ($workspace === null) {
            return [];
        }

        try {
            $report = $this->run($workspace);
        } finally {
            self::remove($workspace);
        }

        if ($report === null) {
            return [];
        }

        $diagnostics = [];

        foreach ($report['issues'] ?? [] as $issue) {
            if (!\in_array($issue['code'] ?? '', self::KEPT, true)) {
                continue;
            }

            $annotation = $issue['annotations'][0] ?? null;
            $start = $annotation['span']['start'] ?? null;

            $diagnostics[] = [
                'level' => \strtolower((string) ($issue['level'] ?? 'error')),
                // The annotation says what was actually wrong ("Expected one of
                // `RightParenthesis`"); the issue message is the generic
                // wrapper. Prefer the specific one.
                'message' => (string) ($annotation['message'] ?? $issue['message'] ?? 'Syntax error.'),
                'offset' => $enriched->originalOffset((int) ($start['offset'] ?? 0)),
                'line' => (int) ($start['line'] ?? 0),
                'help' => isset($issue['help']) ? (string) $issue['help'] : null,
            ];
        }

        return $diagnostics;
    }

    /** A throwaway workspace: Mago works on files in a configured directory. */
    private function workspace(string $php): ?string
    {
        $root = \sys_get_temp_dir() . '/aiwatcher-flow-lint-' . \bin2hex(\random_bytes(8));

        if (!\is_dir($root . '/src') && !\mkdir($root . '/src', 0o700, true)) {
            return null;
        }

        \file_put_contents($root . '/mago.toml', "php-version = \"8.3\"\n[source]\npaths = [\"src\"]\n");
        \file_put_contents($root . '/src/query.php', $php);

        return $root;
    }

    /** @return array{issues?: list<array<string, mixed>>}|null */
    private function run(string $workspace): ?array
    {
        $descriptors = [1 => ['pipe', 'w'], 2 => ['pipe', 'w']];
        $process = \proc_open(
            [$this->binary, 'lint', '--reporting-format=json'],
            $descriptors,
            $pipes,
            $workspace,
            // An empty environment: this runs untrusted text through a parser,
            // and the parser has no reason to see the service's own environment.
            ['PATH' => '/usr/bin:/bin'],
        );

        if (!\is_resource($process)) {
            return null;
        }

        \stream_set_timeout($pipes[1], $this->timeoutSeconds);
        $stdout = \stream_get_contents($pipes[1]);
        \fclose($pipes[1]);
        \fclose($pipes[2]);
        \proc_close($process);

        $decoded = \is_string($stdout) ? \json_decode($stdout, true) : null;

        return \is_array($decoded) ? $decoded : null;
    }

    private static function remove(string $directory): void
    {
        foreach (['/src/query.php', '/mago.toml'] as $file) {
            if (!\is_file($directory . $file)) {
                continue;
            }

            \unlink($directory . $file);
        }

        foreach (['/src', ''] as $path) {
            if (!\is_dir($directory . $path)) {
                continue;
            }

            \rmdir($directory . $path);
        }
    }
}
