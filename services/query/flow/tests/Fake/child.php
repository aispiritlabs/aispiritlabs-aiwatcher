<?php

declare(strict_types=1);

/**
 * A stand-in for `bin/query.php`, for `ChildQueryTest`: what a child does is its first argument.
 *
 *   cancelled <memory> <key> <heartbeat>  asks for its own cancel, as another request would, then works on
 *   stuck <heartbeat>                     works for ever
 *   loud <bytes>                          answers a result that large
 *   dies                                  says something on stderr and exits without an answer
 */

use Aiwatcher\Flow\ExecutionMemory;

require \dirname(__DIR__, 2) . '/vendor/autoload.php';

\stream_get_contents(\STDIN);
$mode = $argv[1] ?? '';

/** Appends a byte every few milliseconds, so a test can tell whether this is still running. */
$work = static function (string $heartbeat): never {
    while (true) {
        \file_put_contents($heartbeat, '.', \FILE_APPEND);
        \usleep(20_000);
    }
};

match ($mode) {
    'cancelled' => (static function () use ($argv, $work): never {
        (new ExecutionMemory($argv[2]))->cancel($argv[3]);
        $work($argv[4]);
    })(),
    'stuck' => $work($argv[2]),
    'loud' => print
        \json_encode([
            'result' => [
                'rows' => [['text' => \str_repeat('x', (int) $argv[2])]],
                'row_count' => 1,
                'digest' => 'loud',
            ],
        ]),
    'dies' => (static function (): never {
        \fwrite(\STDERR, 'PHP Fatal error: Allowed memory size exhausted');
        exit(3);
    })(),
    default => exit(64),
};
