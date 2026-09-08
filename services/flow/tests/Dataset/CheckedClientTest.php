<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\CheckedClient;
use Aiwatcher\Flow\Dataset\UpstreamFailed;
use Nyholm\Psr7\Request;
use Nyholm\Psr7\Response;
use PHPUnit\Framework\TestCase;
use Psr\Http\Client\ClientInterface;
use Psr\Http\Message\RequestInterface;
use Psr\Http\Message\ResponseInterface;

/**
 * What happens when aiwatcher answers something other than rows.
 *
 * The failure this replaces reached a person as `Path "rows" does not exists
 * in array array('code'=>'hubs_disabled','message'=>'thisinstancesearchesno…')`
 * — the sentence that would have fixed it, spelled without its spaces, inside
 * one about an array path. It also came back as a 502, which a managed step
 * reads as "nobody answered" and spends ten attempts on.
 */
final class CheckedClientTest extends TestCase
{
    public function test_an_error_from_aiwatcher_arrives_as_its_own_message_and_not_as_a_missing_array_path(): void
    {
        $client = new CheckedClient(self::answering(
            501,
            '{"error":{"message":"this instance searches no dataset hubs (AIWATCHER_HUGGINGFACE_ENABLED)"}}',
        ));

        try {
            $client->sendRequest(new Request('GET', 'http://aiwatcher/api/v1/dataset-hubs/rows'));
            self::fail('a 501 was let through');
        } catch (UpstreamFailed $error) {
            self::assertStringContainsString('AIWATCHER_HUGGINGFACE_ENABLED', $error->getMessage());
            self::assertStringContainsString('501', $error->getMessage());
            // The route, because "which read failed" is the other half of a
            // chain with several sources in it.
            self::assertStringContainsString('/api/v1/dataset-hubs/rows', $error->getMessage());
        }
    }

    public function test_an_answer_that_will_never_change_is_told_apart_from_one_that_might(): void
    {
        // The whole reason the status is carried. `Transient` costs ten
        // attempts over ten minutes; a 501 naming an unset variable will still
        // be a 501 at the end of them.
        foreach ([400, 404, 422, 501] as $status) {
            self::assertTrue((new UpstreamFailed($status, 'no', 'http://x'))->isPermanent(), (string) $status);
        }
        foreach ([500, 502, 503, 504] as $status) {
            self::assertFalse((new UpstreamFailed($status, 'no', 'http://x'))->isPermanent(), (string) $status);
        }
    }

    public function test_a_body_that_is_not_json_is_reported_by_its_reason_rather_than_quoted(): void
    {
        // A proxy's HTML error page helps nobody, and putting it in a message
        // the panel renders is how a page ends up inside a table cell.
        $client = new CheckedClient(self::answering(503, '<html><body>Service Unavailable</body></html>'));

        $this->expectException(UpstreamFailed::class);
        $this->expectExceptionMessageMatches('/Service Unavailable/');
        $client->sendRequest(new Request('GET', 'http://aiwatcher/api/v1/runs'));
    }

    public function test_a_successful_answer_is_handed_back_untouched(): void
    {
        $client = new CheckedClient(self::answering(200, '{"runs":[]}'));
        $response = $client->sendRequest(new Request('GET', 'http://aiwatcher/api/v1/runs'));

        self::assertSame(200, $response->getStatusCode());
        self::assertSame('{"runs":[]}', (string) $response->getBody());
    }

    private static function answering(int $status, string $body): ClientInterface
    {
        return new class($status, $body) implements ClientInterface {
            public function __construct(
                private readonly int $status,
                private readonly string $body,
            ) {}

            public function sendRequest(RequestInterface $request): ResponseInterface
            {
                return new Response($this->status, ['content-type' => 'application/json'], $this->body);
            }
        };
    }
}
