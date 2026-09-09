<?php

declare(strict_types=1);

/**
 * The Flow query service.
 *
 * Six routes and no framework, because the surface is still deliberately small. The panel
 * talks to this directly rather than through the Rust API: aiwatcher's binary
 * has no idea this exists, which is what lets the service be absent without the
 * rest of the panel noticing (see ADR_0008).
 *
 *   GET  /flow/healthz   is the service up, and can it see aiwatcher
 *   GET  /flow/datasets  what a query may read, and the columns of each
 *   POST /flow/check     {"pipeline": …} -> what is wrong with it, without running it
 *   POST /flow/simulate  {"pipeline": …} -> a 25-row, side-effect-free preview
 *   POST /flow/query     {"pipeline": …, "window_seconds": …, "window_from"/"window_to": …,
 *                         "execution_id": …} -> a table, saying which window it used
 *   GET  /flow/executions/{id}  did this service already run that key
 *
 * The last two are what managed execution needs and all it needs: one field and one
 * route. `execution_id` is
 * `<execution>/<step>/<attempt>`, and what the service remembers about it is that it ran
 * and what the result hashed to — never the rows. ADR 0014 refused this service an S3
 * client and that refusal stands: the rows go to the artifact the *reactor* uploads.
 */

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dataset\CheckedClient;
use Aiwatcher\Flow\Dataset\UpstreamFailed;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\ExecutionMemory;
use Aiwatcher\Flow\Lint\MagoLinter;
use Aiwatcher\Flow\QueryChecker;
use Aiwatcher\Flow\QueryRunner;
use Nyholm\Psr7\Request;
use Symfony\Component\HttpClient\Psr18Client;

require \dirname(__DIR__) . '/vendor/autoload.php';

$aiwatcher = \rtrim((string) (\getenv('AIWATCHER_URL') ?: '') ?: 'http://127.0.0.1:8080', '/');

// Wrapped, so an error from aiwatcher arrives as aiwatcher's own message and
// its own status rather than as Flow failing to find `rows` in an error body.
$client = new CheckedClient(new Psr18Client());
$catalog = new Catalog($client, $aiwatcher);
$linter = MagoLinter::fromVendor($catalog, \dirname(__DIR__));
$runner = new QueryRunner($catalog, $aiwatcher, ExecutionMemory::default());
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
 * The idempotency key of a managed step, from the same body.
 *
 * Absent for every ad-hoc query, which is every query the panel sends: those are not
 * keyed, not resumed and not deduplicated, and nothing about them is remembered.
 */
$executionId = static fn(): ?string => \is_string($request['execution_id'] ?? null)
    && $request['execution_id'] !== ''
        ? $request['execution_id']
        : null;

/**
 * The exact bounds a managed plan pinned, when it pinned any.
 *
 * Sent beside `window_seconds` rather than instead of it: this service narrows a
 * span to the width it is worth, because the aiwatcher API's windowed routes take
 * a duration. What it does *not* do is pretend otherwise — the answer says which
 * of the two the rows were read through. See `QueryRunner::windowApplied`.
 *
 * @return ?array{int, int}
 */
$windowSpan = static function () use ($request): ?array {
    $from = $request['window_from'] ?? null;
    $to = $request['window_to'] ?? null;

    return \is_int($from) && \is_int($to) && $to > $from ? [$from, $to] : null;
};

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

        $path === '/flow/query' && $method === 'POST' => (static function () use (
            $send,
            $runner,
            $pipeline,
            $window,
            $executionId,
            $windowSpan,
        ): void {
            $query = $pipeline();

            if ($query === null) {
                $send(400, ['error' => ['message' => 'Send {"pipeline": "data_frame()->…"}.', 'column' => 0]]);

                return;
            }

            $send(200, $runner->run(
                $query,
                $window(),
                QueryRunner::MAX_ROWS,
                $executionId(),
                $windowSpan(),
            ));
        })(),

        // The lookup half. A reactor asks this after a timeout, before it runs the same
        // key again — a timeout says the caller stopped waiting and nothing about whether
        // this service stopped working. `absent` is the ordinary answer and the safe one.
        \str_starts_with($path, '/flow/executions/') && $method === 'GET' => $send(
            200,
            $runner->seen(\rawurldecode(\substr($path, \strlen('/flow/executions/')))),
        ),

        $path === '/flow/simulate' && $method === 'POST' => (static function () use ($send, $runner, $pipeline, $window): void {
            $query = $pipeline();

            if ($query === null) {
                $send(400, ['error' => ['message' => 'Send {"pipeline": "data_frame()->…"}.', 'column' => 0]]);

                return;
            }

            $send(200, $runner->run($query, $window(), QueryRunner::SIMULATION_ROWS));
        })(),

        default => $send(404, ['error' => ['message' => \sprintf('No route %s.', $path), 'column' => 0]]),
    };
} catch (ParseError $error) {
    // 422, not 400: the request was well-formed, the query was not. The column
    // is what lets the panel point at the character instead of the query.
    $send(422, ['error' => $error->toArray()]);
} catch (UpstreamFailed $error) {
    // aiwatcher answered, and its answer decides this one. A 501 naming an
    // unset variable is relayed as a 4xx so the caller reads it as permanent:
    // a managed step classifies a 5xx from here as "nobody answered" and
    // spends ten attempts over ten minutes discovering that a configuration
    // flag is still off. Everything else stays 502, which is the honest
    // reading of a store that may come back.
    $send($error->isPermanent() ? 422 : 502, ['error' => [
        'message' => \sprintf('The query could not be run: %s', $error->getMessage()),
        'column' => 0,
    ]]);
} catch (\Throwable $error) {
    // Anything else is aiwatcher being unreachable, or a bug here. Both are
    // worth saying plainly rather than as an empty table.
    $send(502, ['error' => [
        'message' => \sprintf('The query could not be run: %s', $error->getMessage()),
        'column' => 0,
    ]]);
}
