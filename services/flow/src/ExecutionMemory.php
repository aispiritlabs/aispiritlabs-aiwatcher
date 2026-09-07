<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

/**
 * What this service remembers about a managed execution: that it ran, and what the
 * result hashed to. Never the rows.
 *
 * Section 15.4 of `docs/PIPELINE_ARCHITECTURE.md`. A reactor's timeout proves nothing
 * about whether a query finished, so before it runs one again it asks by the idempotency
 * key `<execution>/<step>/<attempt>`. Three answers, and each sends the reactor somewhere
 * different:
 *
 *   running  the query is still executing here — wait, do not send it again
 *   done     it finished, and this is what it hashed to
 *   absent   this service has no record of that key — running it again is safe
 *
 * ADR 0014 refused this service an S3 client, and that refusal stands: the rows go to the
 * artifact the *reactor* uploads, and what stays here is a note. So `done` answers "it
 * ran"; "here is what it produced" is a different question, answered by the object store
 * the reactor wrote to.
 *
 * ## Why the notes are files
 *
 * "In memory, for minutes" is the shape, and a PHP process has no memory between requests.
 * `php -S` forks workers and PHP-FPM forks children, so a static array would answer
 * `absent` to whichever worker happened to take the lookup — which is the bug this route
 * exists to remove, arriving from a different direction.
 *
 * Files in the system temp directory have the property that matters and no more of it: the
 * note lives as long as the lookup window and is scoped to one host, so a second replica
 * behind a load balancer answers `absent` for the other replica's execution. That is
 * exactly what section 15.4 says it should do, and it is safe — the reactor treats
 * `absent` as "retry", and a retry of a deterministic query over the same window writes
 * the same digest.
 */
final readonly class ExecutionMemory
{
    /**
     * How long a note is worth reading.
     *
     * The reactor's lookup window: it asks once, immediately after a timeout it did not
     * expect. Minutes rather than hours because a note that outlived the attempt it
     * describes would answer for the *next* attempt at the same key — and there is no next
     * attempt at the same key, because the attempt number is in it.
     */
    public const int TTL_SECONDS = 900;

    /** A running query whose worker died leaves a note nothing will ever complete. */
    public const string RUNNING = 'running';

    public const string DONE = 'done';

    public const string ABSENT = 'absent';

    public function __construct(
        private string $directory,
    ) {}

    /**
     * The default location: one directory per host, under the system temp directory.
     *
     * `0700` because the notes name executions, and a service with no authentication of its
     * own should at least not publish what it ran to every user on the box.
     */
    public static function default(): self
    {
        $directory = \rtrim(\sys_get_temp_dir(), '/') . '/aiwatcher-flow-executions';

        if (!\is_dir($directory)) {
            self::quietly(static fn(): bool => \mkdir($directory, 0o700, true));
        }

        return new self($directory);
    }

    /**
     * Note that a query is running, before it runs.
     *
     * Before rather than after: the whole value of `running` is that it is true while the
     * query is executing, which is the window a reactor's timeout falls in.
     */
    public function started(string $executionId): void
    {
        $this->write($executionId, ['state' => self::RUNNING]);
    }

    /** Note what it hashed to, and how many rows there were. */
    public function finished(string $executionId, string $digest, int $rows): void
    {
        $this->write($executionId, ['state' => self::DONE, 'digest' => $digest, 'rows' => $rows]);
    }

    /**
     * Forget a query that ended badly.
     *
     * A failed query must not leave a `running` note behind: the reactor would wait for
     * something that stopped, and only the lease expiring would free it. `absent` is the
     * honest answer — nothing here ran to completion, so running it again is safe.
     */
    public function failed(string $executionId): void
    {
        $path = $this->path($executionId);
        self::quietly(static fn(): bool => \unlink($path));
    }

    /**
     * What this service knows about that key.
     *
     * @return array{state: string, digest?: string, rows?: int}
     */
    public function seen(string $executionId): array
    {
        $path = $this->path($executionId);
        $raw = self::quietly(static fn(): string|false => \file_get_contents($path));

        if (!\is_string($raw)) {
            return ['state' => self::ABSENT];
        }

        $note = \json_decode($raw, true);

        if (!\is_array($note) || !\is_string($note['state'] ?? null)) {
            return ['state' => self::ABSENT];
        }

        // Expiry is read rather than swept: a sweep would need a lock, and a note nobody
        // reads costs a few hundred bytes until the operating system clears the directory.
        if (!\is_int($note['at'] ?? null) || (\time() - $note['at']) > self::TTL_SECONDS) {
            self::quietly(static fn(): bool => \unlink($path));

            return ['state' => self::ABSENT];
        }

        unset($note['at']);

        /** @var array{state: string, digest?: string, rows?: int} $note */
        return $note;
    }

    /**
     * The fingerprint this service reports for a result.
     *
     * Its own encoding's digest, and deliberately not the reactor's. The two sides encode
     * JSON differently — key order, float formatting, unicode escaping — and a
     * cross-language byte-for-byte contract over row data is one nobody could keep. What
     * this is for is the one comparison that is meaningful: whether the note beside the
     * stored artifact describes *this* run of this key.
     *
     * @param list<array<string, mixed>> $rows
     */
    public static function digestOf(array $rows): string
    {
        return \hash('sha256', \json_encode($rows, \JSON_THROW_ON_ERROR | \JSON_UNESCAPED_SLASHES));
    }

    /** @param array{state: string, digest?: string, rows?: int} $note */
    private function write(string $executionId, array $note): void
    {
        $note['at'] = \time();
        $encoded = \json_encode($note, \JSON_THROW_ON_ERROR);

        // Written whole, then moved: a lookup that read a half-written file would decode to
        // nothing and answer `absent`, which is safe but would make `running` unreliable
        // exactly when it matters.
        $temporary = $this->path($executionId) . '.' . \getmypid() . '.tmp';
        $final = $this->path($executionId);

        $written = self::quietly(static fn(): int|false => \file_put_contents($temporary, $encoded, \LOCK_EX));

        if (\is_int($written)) {
            self::quietly(static fn(): bool => \rename($temporary, $final));
        }
    }

    /**
     * A filesystem call whose failure is an answer rather than an error.
     *
     * Every write here is best-effort by design: a note this service could not store means
     * the next lookup answers `absent`, which is the safe answer and the one a reactor
     * already handles. What must never happen is a *query* failing because a note about it
     * could not be written — so the warning is swallowed deliberately, in one place, rather
     * than with an `@` at each call site that a reader has to notice.
     *
     * @template T
     *
     * @param callable(): T $call
     *
     * @return T
     */
    private static function quietly(callable $call): mixed
    {
        \set_error_handler(static fn(): bool => true);

        try {
            return $call();
        } finally {
            \restore_error_handler();
        }
    }

    /**
     * The key hashed, because its parts are a caller's strings.
     *
     * `<execution>/<step>/<attempt>` holds slashes, and a step id comes from a canvas. A
     * path built from it verbatim would be a traversal in a service that has no
     * authentication of its own.
     */
    private function path(string $executionId): string
    {
        return $this->directory . '/' . \hash('sha256', $executionId) . '.json';
    }
}
