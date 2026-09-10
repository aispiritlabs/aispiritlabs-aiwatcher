<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\FlowAI;

use Aiwatcher\Flow\FlowAI\FitPreparation;
use Aiwatcher\Flow\FlowAI\FittedState;
use Aiwatcher\Flow\FlowAI\OneHotEncoder;
use Aiwatcher\Flow\FlowAI\Preparation;
use Aiwatcher\Flow\FlowAI\Scalar\OneHot;
use Flow\ETL\Row;
use Flow\ETL\Transformer\ScalarFunctionTransformer;
use PHPUnit\Framework\TestCase;

use function Flow\ETL\DSL\data_frame;
use function Flow\ETL\DSL\datetime_entry;
use function Flow\ETL\DSL\flow_context;
use function Flow\ETL\DSL\from_array;
use function Flow\ETL\DSL\int_entry;
use function Flow\ETL\DSL\ref;
use function Flow\ETL\DSL\rows;
use function Flow\ETL\DSL\str_entry;

final class ScalarPreparationTest extends TestCase
{
    public function test_fit_keeps_the_exact_rows_and_scalar_preserves_unrelated_typed_entries(): void
    {
        $context = flow_context();
        $date = datetime_entry('created_at', new \DateTimeImmutable('2026-09-09T12:00:00Z'));
        $input = rows(Row::create(str_entry('category', 'a'), $date, int_entry('id', 7)));
        $state = new FittedState();
        $fit = new FitPreparation('oneHotEncode', ['category'], [], $state);
        self::assertSame($input, $fit->transform($input, $context));
        $function = new OneHot([ref('category')], $state);
        $result = (new ScalarFunctionTransformer('encoded', $function))->transform($input, $context);
        self::assertSame($date, $result->first()->get('created_at'));
        self::assertSame($input->first()->get('id'), $result->first()->get('id'));
        self::assertSame(['category="a"' => 1.0], $result->first()->valueOf('encoded'));
        self::assertSame(['category="a"' => 1.0], \unserialize(\serialize($function))->eval($input->first(), $context));
    }

    public function test_unfitted_expression_graph_serializes_with_shared_fit_state(): void
    {
        $state = new FittedState();
        [$fit, $function] = \unserialize(\serialize([
            new FitPreparation('oneHotEncode', ['x'], [], $state),
            new OneHot([ref('x')], $state),
        ]));
        $input = rows(Row::create(str_entry('x', 'train')));
        $fit->transform($input, flow_context());
        self::assertSame(['x="train"' => 1.0], $function->eval($input->first(), flow_context()));
        // A fresh execution replaces its fitted parameters, never accumulates vocabulary.
        $other = rows(Row::create(str_entry('x', 'other')));
        $fit->transform($other, flow_context());
        self::assertSame(['x="other"' => 1.0], $function->eval($other->first(), flow_context()));
    }

    public function test_inference_yields_without_collecting_the_input(): void
    {
        $state = (new OneHotEncoder(['x']))->fit([['x' => 'known']])->toState();
        $source = (static function (): \Generator {
            yield ['x' => 'unknown', 'id' => 1];
            // Flow 0.43 Segment advances the source once before yielding output.
            yield ['x' => 'known', 'id' => 2];
            throw new \RuntimeException('input was collected beyond native one-page lookahead');
        })();
        $frame = (new Preparation(
            'oneHotEncode',
            ['x'],
            ['output' => 'features', 'state' => \json_encode($state, \JSON_THROW_ON_ERROR)],
        ))->apply(data_frame()->read(from_array($source)));
        $pages = $frame->get();
        self::assertTrue($pages->valid(), 'no inference page was produced');
        self::assertSame(['x="known"' => 0.0], $pages->current()->first()->valueOf('features'));
    }

    public function test_in_place_imputation_keeps_the_original_missing_indicator(): void
    {
        $frame = (new Preparation(
            'imputeMissing',
            ['age'],
            ['output' => 'age', 'missingIndicator' => 'was_missing'],
        ))->apply(data_frame()->read(from_array([['age' => 20], ['age' => null]])));
        $result = $frame->fetch()->toArray();
        self::assertEquals(20, $result[1]['age']);
        self::assertSame(1, $result[1]['was_missing']);
        self::assertSame(0, $result[0]['was_missing']);
    }
}
