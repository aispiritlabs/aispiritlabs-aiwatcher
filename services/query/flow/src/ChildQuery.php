<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

/**
 * A managed query, run in a process of its own so a cancel can end it wherever it is.
 *
 * A request is a thread of FrankenPHP or a worker of `php -S`, and neither can be killed from
 * another request — so a query run inside one could only be told to stop, and looked for the
 * marker between the batches it read. A single long read or a final aggregation has no
 * "between": it ran on to its own time limit after its run was cancelled, holding a thread.
 *
 * A child process has a pid. The request waits on it, reads its answer as it arrives so a
 * large one never fills the pipe, and looks for the marker `ExecutionMemory::cancel` writes
 * every tenth of a second; on a mark, or past the limit on the wall clock, it kills the child
 * and answers as a query that stopped. The cost is one PHP start per managed query — tens of
 * milliseconds against a step measured in seconds — and an ad-hoc query from the panel pays
 * nothing, because nothing can cancel a query that has no key.
 */
final readonly class ChildQuery
{
    /** How often the request looks for a mark while the child works. */
    public const int POLL_MICROSECONDS = 100_000;

    /**
     * How long the request outlives its own limit.
     *
     * Under FrankenPHP `max_execution_time` is a wall clock, and a request that reached it
     * would die as a fatal without killing the child. So the request is given a little more
     * than the limit it enforces, and the child is always killed by this class.
     */
    private const int GRACE_SECONDS = 5;

    /** The most of what the child said on stderr that a failure repeats. */
    private const int COMPLAINT_BYTES = 2_048;

    /**
     * @param list<string>                $command     the child's argv, never a shell line
     * @param ?array<string, string>      $environment null inherits this process's
     */
    public function __construct(
        private array $command,
        private ExecutionMemory $memory,
        private int $timeoutSeconds,
        private ?array $environment = null,
    ) {}

    /**
     * `bin/query.php` under the PHP that serves this request.
     *
     * Under FrankenPHP `PHP_BINARY` is the server, not an interpreter, and the image ships the
     * CLI beside it as `php`.
     *
     * @param ?array<string, string> $environment
     */
    public static function fromRoot(
        string $root,
        ExecutionMemory $memory,
        int $timeoutSeconds,
        ?array $environment = null,
    ): self {
        $php = \PHP_SAPI === 'frankenphp' ? 'php' : \PHP_BINARY;

        return new self([$php, $root . '/bin/query.php'], $memory, $timeoutSeconds, $environment);
    }

    /**
     * Run one request in the child and answer what `QueryRunner::run` would have.
     *
     * @param array{pipeline: string, window_seconds: ?int, max_rows: int, window_span: ?array{int, int}} $request
     *
     * @throws QueryCancelled when the run it belongs to stopped
     * @throws QueryFailed    when the query failed in the child
     *
     * @return array<string, mixed>
     */
    public function run(string $executionId, array $request): array
    {
        \set_time_limit($this->timeoutSeconds + self::GRACE_SECONDS);

        $pipes = [];
        $process = \proc_open(
            $this->command,
            [0 => ['pipe', 'r'], 1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
            $pipes,
            null,
            $this->environment,
        );

        if (!\is_resource($process)) {
            throw new \RuntimeException('its process did not start');
        }

        [$input, $output, $errors] = [$pipes[0], $pipes[1], $pipes[2]];
        \fwrite($input, \json_encode($request, \JSON_THROW_ON_ERROR | \JSON_UNESCAPED_SLASHES));
        \fclose($input);
        \stream_set_blocking($output, false);
        \stream_set_blocking($errors, false);

        $answer = '';
        $complaint = '';
        $deadline = \microtime(true) + $this->timeoutSeconds;
        $ended = false;

        try {
            while (!\feof($output) || !\feof($errors)) {
                $readable = [$output, $errors];
                $unused = null;
                \stream_select($readable, $unused, $unused, 0, self::POLL_MICROSECONDS);

                foreach ($readable as $stream) {
                    $chunk = (string) \stream_get_contents($stream);
                    $answer .= $stream === $output ? $chunk : '';
                    $complaint = $stream === $errors
                        ? \substr($complaint . $chunk, -self::COMPLAINT_BYTES)
                        : $complaint;
                }

                if ($this->memory->cancelled($executionId)) {
                    throw new QueryCancelled($executionId);
                }

                if (\microtime(true) > $deadline) {
                    throw new \RuntimeException(\sprintf(
                        'it ran past its %d-second limit and was stopped',
                        $this->timeoutSeconds,
                    ));
                }
            }

            $ended = true;
        } finally {
            if (!$ended) {
                // SIGKILL: a query that was asked to stop has nothing left worth finishing.
                \proc_terminate($process, 9);
            }

            \fclose($output);
            \fclose($errors);
            $status = \proc_close($process);
        }

        return self::answered($answer, $status, $complaint);
    }

    /** @return array<string, mixed> */
    private static function answered(string $answer, int $status, string $complaint): array
    {
        $decoded = \json_decode($answer, true);

        if (\is_array($decoded) && \is_array($decoded['result'] ?? null)) {
            /** @var array<string, mixed> */
            return $decoded['result'];
        }

        $failure = \is_array($decoded) ? $decoded['failure'] ?? null : null;

        if (\is_array($failure) && \is_int($failure['status'] ?? null) && \is_array($failure['error'] ?? null)) {
            /** @var array<string, mixed> $error */
            $error = $failure['error'];

            throw new QueryFailed($failure['status'], $error);
        }

        // A fatal error — memory, a segfault — writes no answer. What it said is the only
        // explanation there is, from whichever stream the interpreter's settings sent it to.
        $said = \trim($complaint !== '' ? $complaint : \substr($answer, -self::COMPLAINT_BYTES));

        throw new \RuntimeException(\sprintf(
            'its process ended with status %d and no answer%s',
            $status,
            $said === '' ? '' : ': ' . $said,
        ));
    }
}
