<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\FlowAI;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\FlowAI\LabelEncoder;
use Aiwatcher\Flow\FlowAI\OneHotEncoder;
use Aiwatcher\Flow\FlowAI\Preparation;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

use function Flow\ETL\DSL\data_frame;
use function Flow\ETL\DSL\from_array;

final class PreparationTest extends TestCase
{
    public function test_scalar_categories_and_state_roundtrip(): void
    {
        $rows = [['x' => null], ['x' => 'null'], ['x' => 1], ['x' => '1']];
        $encoder = (new OneHotEncoder(['x']))->fit($rows);
        self::assertCount(4, $encoder->transform($rows)[0]);
        self::assertSame($encoder->transform($rows), OneHotEncoder::fromState($encoder->toState())->transform($rows));
        self::assertSame(0.0, \array_sum($encoder->transform([['x' => 'unseen']])[0]));
        $labels = (new LabelEncoder())->fit([1, 0, 1]);
        self::assertSame([1, 0], LabelEncoder::fromState($labels->toState())->inverseTransform([1, 0]));
    }

    public function test_validation_does_not_influence_fit_and_unknowns_are_explicit(): void
    {
        $rows = [['x' => 'train-only', '_split' => 'train'], ['x' => 'validation-only', '_split' => 'validation']];
        $step = new Preparation(
            'oneHotEncode',
            ['x'],
            ['fitOn' => '_split', 'output' => 'features', 'stateOutput' => 'state'],
        );
        $result = $this->apply($step, $rows);
        self::assertSame(['x="train-only"' => 0.0], $result[1]['features']);
        self::assertSame([['"train-only"']], $result[0]['state']['categories']);
        $inference = new Preparation(
            'oneHotEncode',
            ['x'],
            ['output' => 'features', 'state' => \json_encode($result[0]['state'], \JSON_THROW_ON_ERROR)],
        );
        self::assertSame($result[1]['features'], $this->apply($inference, [$rows[1]])[0]['features']);
        $this->expectException(\InvalidArgumentException::class);
        (new OneHotEncoder(['x'], 'error'))->fit([$rows[0]])->transform([$rows[1]]);
    }

    public function test_grouped_imputation_uses_training_and_global_fallback(): void
    {
        $step = new Preparation(
            'imputeMissing',
            ['age'],
            ['fitOn' => 'split', 'groupBy' => 'group', 'output' => 'filled', 'missingIndicator' => 'missing'],
        );
        $rows = [
            ['age' => 20, 'group' => 'A', 'split' => 'train'],
            ['age' => 40, 'group' => 'A', 'split' => 'train'],
            ['age' => 999, 'group' => 'B', 'split' => 'validation'],
            ['age' => null, 'group' => 'B', 'split' => 'validation'],
        ];
        self::assertEquals(30, $this->apply($step, $rows)[3]['filled']);
        self::assertSame(1, $this->apply($step, $rows)[3]['missing']);
        $rows[2]['age'] = -999;
        self::assertEquals(30, $this->apply($step, $rows)[3]['filled']);
        self::assertSame(1, $this->apply($step, $rows)[3]['missing']);
    }

    public function test_split_is_stratified_and_stable_under_row_reordering(): void
    {
        $rows = \array_map(static fn(int $id): array => ['id' => $id, 'target' => $id % 2], \range(1, 20));
        $step = new Preparation(
            'trainTestSplit',
            ['id'],
            ['target' => 'target', 'output' => '_split', 'seed' => 42, 'fraction' => 0.2],
        );
        $split = $this->apply($step, $rows);
        self::assertSame(
            4,
            \count(\array_filter($split, static fn(array $row): bool => $row['_split'] === 'validation')),
        );
        self::assertSame($split, \array_reverse($this->apply($step, \array_reverse($rows))));
        $this->expectExceptionMessage('unique');
        $this->apply($step, [$rows[0], $rows[0]]);
    }

    public function test_dsl_fits_across_pages_and_registers_new_columns(): void
    {
        $catalog = new Catalog(FakeApi::withDemoRuns(), 'http://api.test');
        $plan = (new PipelineBuilder($catalog))->build(Parser::parse(
            "data_frame()->read(default)
            ->oneHotEncode('run_id', stateOutput: 'encoder')
            ->labelEncode('status')
            ->select(ref('model_features'), ref('target_encoded'), ref('encoder'))",
        ));
        $rows = $plan->frame->fetch()->toArray();
        self::assertCount(3, $rows);
        self::assertSame(['"run-1"', '"run-2"', '"run-3"'], $rows[0]['encoder']['categories'][0]);
        self::assertEquals(1, $rows[2]['model_features']['run_id="run-3"']);
        self::assertSame(0, $rows[2]['target_encoded']);
    }

    public function test_dsl_refuses_unknown_options_before_execution(): void
    {
        $this->expectExceptionMessage('Unknown or duplicate FlowAI option');
        (new PipelineBuilder(new Catalog(FakeApi::withDemoRuns(), 'http://api.test')))->build(Parser::parse(
            "data_frame()->read(default)->oneHotEncode('status', callback: 'system')",
        ));
    }

    private function apply(Preparation $step, array $rows): array
    {
        return $step
            ->apply(data_frame()->read(from_array($rows)))
            ->fetch()
            ->toArray();
    }
}
