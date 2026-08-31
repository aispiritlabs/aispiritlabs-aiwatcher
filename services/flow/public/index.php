<?php

declare(strict_types=1);

/**
 * The Flow query service.
 *
 * Four routes and no framework, because there are four routes. The panel
 * talks to this directly rather than through the Rust API: aiwatcher's binary
 * has no idea this exists, which is what lets the service be absent without the
 * rest of the panel noticing (see ADR_0008).
 *
 *   GET  /flow/healthz   is the service up, and can it see aiwatcher
 *   GET  /flow/datasets  what a query may read, and the columns of each
 *   POST /flow/check     {"pipeline": …} -> what is wrong with it, without running it
 *   POST /flow/query     {"pipeline": …, "window_seconds": …} -> a table
 */

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Lint\MagoLinter;
use Aiwatcher\Flow\QueryChecker;
use Aiwatcher\Flow\QueryRunner;
use Nyholm\Psr7\Request;
use Symfony\Component\HttpClient\Psr18Client;

require \dirname(__DIR__) . '/vendor/autoload.php';

$aiwatcher = \rtrim((string) (\getenv('AIWATCHER_URL') ?: '') ?: 'http://127.0.0.1:8080', '/');

$client = new Psr18Client();
$catalog = new Catalog($client, $aiwatcher);
$linter = MagoLinter::fromVendor($catalog, \dirname(__DIR__));
$runner = new QueryRunner($catalog, $aiwatcher);
$checker = new QueryChecker($catalog, $linter);

/** The request body, decoded once — `php://input` is read, not re-read. */
$request = (static function (): array {
    $raw = \file_get_contents('php://input');
    $body = \json_decode(\is_string($raw) ? $raw : '', true);

    return \is_array($body) ? $body : [];
})();

/** Read a `{"pipeline": "…"}` body, or null when it is not one. */
$pipeline = static fn(): ?string => \is_string($request['pipeline'] ?? null) ? $request['pipeline'] : null;

/**
 * The panel's time window, in seconds, from the same body.
 *
 * Anything that is not a positive whole number reads as "everything" rather
 * than as an error: the window is a view control, and a malformed one should
 * widen the answer, never refuse to give it.
 */
$window = static function () use ($request): ?int {
    $value = $request['window_seconds'] ?? null;

    return \is_int($value) && $value > 0 ? $value : null;
};

$path = \parse_url($_SERVER['REQUEST_URI'] ?? '/', \PHP_URL_PATH) ?: '/';
$method = $_SERVER['REQUEST_METHOD'] ?? 'GET';

/** @param array<string, mixed> $body */
$send = static function (int $status, array $body): void {
    \http_response_code($status);
    \header('content-type: application/json');
    echo \json_encode($body, \JSON_THROW_ON_ERROR | \JSON_UNESCAPED_SLASHES | \JSON_UNESCAPED_UNICODE);
};

try {
    match (true) {
        $path === '/flow/healthz' => $send(200, [
            'status' => 'ok',
            'aiwatcher' => $aiwatcher,
            // Whether query checking has the linter behind it. Absent is a
            // normal state — `composer install --no-dev` leaves it out — and
            // checks still run, with the parser's diagnostics alone.
            'linter' => $linter->available() ? 'mago' : 'none',
            // Whether *this* service is up is rarely the question; whether it
            // can reach aiwatcher is. Reported separately so a panel showing
            // an empty table can say which of the two is missing.
            'aiwatcher_reachable' => (static function () use ($aiwatcher, $client): bool {
                try {
                    return $client->sendRequest(new Request('GET', $aiwatcher . '/livez'))
                        ->getStatusCode() < 400;
                } catch (\Throwable) {
                    return false;
                }
            })(),
        ]),

        $path === '/flow/datasets' => $send(200, $runner->datasets()),

        $path === '/flow/check' && $method === 'POST' => (static function () use ($send, $checker, $pipeline): void {
            $query = $pipeline();

            if ($query === null) {
                $send(400, ['error' => ['message' => 'Send {"pipeline": "data_frame()->…"}.', 'column' => 0]]);

                return;
            }

            // Always 200: "this query is wrong" is a successful check, not a
            // failed request. The editor reads `ok`.
            $send(200, $checker->check($query));
        })(),

        $path === '/flow/query' && $method === 'POST' => (static function () use ($send, $runner, $pipeline, $window): void {
            $query = $pipeline();

            if ($query === null) {
                $send(400, ['error' => ['message' => 'Send {"pipeline": "data_frame()->…"}.', 'column' => 0]]);

                return;
            }

            $send(200, $runner->run($query, $window()));
        })(),

        default => $send(404, ['error' => ['message' => \sprintf('No route %s.', $path), 'column' => 0]]),
    };
} catch (ParseError $error) {
    // 422, not 400: the request was well-formed, the query was not. The column
    // is what lets the panel point at the character instead of the query.
    $send(422, ['error' => $error->toArray()]);
} catch (\Throwable $error) {
    // Anything else is aiwatcher being unreachable, or a bug here. Both are
    // worth saying plainly rather than as an empty table.
    $send(502, ['error' => [
        'message' => \sprintf('The query could not be run: %s', $error->getMessage()),
        'column' => 0,
    ]]);
}
