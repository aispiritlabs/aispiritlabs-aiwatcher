<?php

declare(strict_types=1);

namespace Aiwatcher\Flow;

use Aiwatcher\Flow\Dataset\Catalog;
use Psr\Http\Client\ClientInterface;

/**
 * What this service reads from its environment, read in one place.
 *
 * Two entry points read it — the request (`public/index.php`) and the process a managed query
 * runs in (`bin/query.php`) — and a query answered in the second has to read exactly the
 * catalog the first would have. Two copies of these lines would be free to disagree about a
 * default.
 */
final readonly class Configuration
{
    public function __construct(
        /** The aiwatcher API every dataset but the corpus is read from. */
        public string $aiwatcher,
        /**
         * A corpus on disk, for `corpus_spans`. Unset — the default — the catalog is the
         * aiwatcher API and nothing else, as ADR_0008 has it; set, one more dataset reads the
         * part files under it. The benchmark in `benchmarks/curation` is what sets it, beside a
         * time limit sized for reading them.
         */
        public ?string $corpus,
        public int $timeoutSeconds,
    ) {}

    public static function fromEnvironment(): self
    {
        $timeout = \filter_var(\getenv('AIWATCHER_QUERY_TIMEOUT_SECONDS'), \FILTER_VALIDATE_INT, [
            'options' => ['min_range' => 1],
        ]);
        $corpus = \getenv('AIWATCHER_CORPUS_DIR');
        $aiwatcher = \getenv('AIWATCHER_URL');

        return new self(
            \rtrim(\is_string($aiwatcher) && $aiwatcher !== '' ? $aiwatcher : 'http://127.0.0.1:8080', '/'),
            \is_string($corpus) && $corpus !== '' ? $corpus : null,
            \is_int($timeout) ? $timeout : QueryRunner::TIMEOUT_SECONDS,
        );
    }

    public function catalog(ClientInterface $client): Catalog
    {
        return new Catalog($client, $this->aiwatcher, corpusRoot: $this->corpus);
    }
}
