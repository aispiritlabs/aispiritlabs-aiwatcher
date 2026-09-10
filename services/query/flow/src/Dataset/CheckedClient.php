<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

use Psr\Http\Client\ClientInterface;
use Psr\Http\Message\RequestInterface;
use Psr\Http\Message\ResponseInterface;

/**
 * The client the catalog reads aiwatcher through, with the status checked.
 *
 * A decorator rather than a check inside the pipeline, because the pipeline is
 * lazy and declarative: by the time `array_get(__body, 'rows')` runs there is
 * no status left to branch on, only a body that does not have the key. The
 * moment the answer arrives is the only place that knows both.
 *
 * It is also the one place that can be tested without Flow: a stub client, a
 * response, an assertion about what comes out.
 */
final class CheckedClient implements ClientInterface
{
    public function __construct(
        private readonly ClientInterface $inner,
    ) {}

    public function sendRequest(RequestInterface $request): ResponseInterface
    {
        $response = $this->inner->sendRequest($request);
        $status = $response->getStatusCode();

        if ($status < 200 || $status >= 300) {
            throw new UpstreamFailed($status, self::detailOf($response), (string) $request->getUri());
        }

        return $response;
    }

    /**
     * aiwatcher's own words, in whichever shape this route answers with.
     *
     * `{"error":{"message":…}}` is what the API's error body looks like and
     * `{"message":…}` is what a couple of the older ones send. The reason
     * phrase is the fallback rather than the body, because a body that is not
     * JSON is usually a proxy's HTML and quoting it helps nobody.
     */
    private static function detailOf(ResponseInterface $response): string
    {
        $body = (string) $response->getBody();
        $decoded = \json_decode($body, true);

        if (\is_array($decoded)) {
            if (isset($decoded['error']['message']) && \is_string($decoded['error']['message'])) {
                return $decoded['error']['message'];
            }
            if (isset($decoded['message']) && \is_string($decoded['message'])) {
                return $decoded['message'];
            }
        }

        $reason = $response->getReasonPhrase();

        return $reason === '' ? 'no message' : $reason;
    }
}
