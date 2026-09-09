<?php

declare(strict_types=1);

// Run the authored FlowPHP graph over a local CSV using the same paginated catalog contract.
require __DIR__ . '/../../services/flow/vendor/autoload.php';

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Nyholm\Psr7\Response;
use Psr\Http\Client\ClientInterface;
use Psr\Http\Message\RequestInterface;
use Psr\Http\Message\ResponseInterface;

if ($argc < 2) {
    fwrite(STDERR, "Usage: php examples/titanic/prepare.php train.csv [limit]\n");
    exit(1);
}
$stream = fopen($argv[1], 'rb');
$header = fgetcsv($stream, escape: '');
$passengers = [];
$limit = isset($argv[2]) ? (int) $argv[2] : 891;
while (count($passengers) < $limit && ($values = fgetcsv($stream, escape: '')) !== false) {
    $row = array_combine($header, $values);
    foreach ($row as $column => $value) {
        $row[$column] = match (true) {
            $value === '' => null,
            in_array($column, ['PassengerId', 'Survived', 'Pclass', 'SibSp', 'Parch'], true) => (int) $value,
            in_array($column, ['Age', 'Fare'], true) => (float) $value,
            default => $value,
        };
    }
    $passengers[] = ['row_index' => count($passengers), 'row' => $row, 'omitted' => []];
}
fclose($stream);
$api = new class($passengers) implements ClientInterface {
    public function __construct(private array $rows) {}

    public function sendRequest(RequestInterface $request): ResponseInterface
    {
        parse_str($request->getUri()->getQuery(), $query);
        return new Response(200, ['content-type' => 'application/json'], json_encode([
            'rows' => array_slice($this->rows, (int) ($query['offset'] ?? 0), (int) ($query['limit'] ?? 100)),
            'total_rows' => count($this->rows),
        ], JSON_THROW_ON_ERROR));
    }
};
$graph = json_decode(file_get_contents(__DIR__ . '/pipeline-php.json'), true, 512, JSON_THROW_ON_ERROR);
$query = "data_frame()->read(hub_rows, dataset: 'phihung/titanic', split: 'train', limit: 891)\n";
foreach ($graph['blocks'] as $block) {
    if ($block['spec']['kind'] === 'transform') {
        $query .= $block['spec']['steps'] . "\n";
    }
}
$plan = (new PipelineBuilder(new Catalog($api, 'http://local-csv.test')))->build(Parser::parse($query));
echo json_encode($plan->frame->fetch()->toArray(), JSON_THROW_ON_ERROR | JSON_PRETTY_PRINT), "\n";
