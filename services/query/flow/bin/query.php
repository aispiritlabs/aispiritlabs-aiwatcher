<?php

declare(strict_types=1);

/**
 * One managed query, in a process of its own: its request on stdin, its answer on stdout.
 *
 * `ChildQuery` starts this and nothing else should. It reads the environment the request
 * would have (`Configuration`), runs the query the request would have run, and writes either
 * `{"result": …}` — exactly `QueryRunner::run`'s answer — or `{"failure": {"status", "error"}}`,
 * decided by `Failure::of` here rather than again in the request. It keeps no note: the
 * request that waits on it owns the key, and is what a cancel reaches.
 *
 * Outside `public/` on purpose: a file there is a route.
 */

use Aiwatcher\Flow\Configuration;
use Aiwatcher\Flow\Dataset\CheckedClient;
use Aiwatcher\Flow\Failure;
use Aiwatcher\Flow\QueryRunner;
use Symfony\Component\HttpClient\Psr18Client;

require \dirname(__DIR__) . '/vendor/autoload.php';

$configuration = Configuration::fromEnvironment();
$raw = \stream_get_contents(\STDIN);
$request = \json_decode(\is_string($raw) ? $raw : '', true);

try {
    if (!\is_array($request) || !\is_string($request['pipeline'] ?? null)) {
        throw new \InvalidArgumentException('the request it was handed is not one');
    }

    $span = $request['window_span'] ?? null;
    $runner = new QueryRunner(
        $configuration->catalog(new CheckedClient(new Psr18Client())),
        $configuration->aiwatcher,
        timeoutSeconds: $configuration->timeoutSeconds,
    );
    $answer = ['result' => $runner->run(
        $request['pipeline'],
        \is_int($request['window_seconds'] ?? null) ? $request['window_seconds'] : null,
        \is_int($request['max_rows'] ?? null) ? $request['max_rows'] : QueryRunner::MAX_ROWS,
        null,
        \is_array($span) && \is_int($span[0] ?? null) && \is_int($span[1] ?? null) ? [$span[0], $span[1]] : null,
    )];
} catch (\Throwable $error) {
    $failure = Failure::of($error);
    $answer = ['failure' => ['status' => $failure->status, 'error' => $failure->error]];
}

echo \json_encode($answer, \JSON_THROW_ON_ERROR | \JSON_UNESCAPED_SLASHES | \JSON_UNESCAPED_UNICODE);
