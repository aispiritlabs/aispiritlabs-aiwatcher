<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Dataset;

use Aiwatcher\Flow\Dataset\Catalog;
use Aiwatcher\Flow\Dsl\ParseError;
use Aiwatcher\Flow\Dsl\Parser;
use Aiwatcher\Flow\Dsl\PipelineBuilder;
use Aiwatcher\Flow\Tests\Fake\FakeApi;
use PHPUnit\Framework\TestCase;

/**
 * Reading a public dataset hub, and reading what a project already holds.
 *
 * Two things are worth defending here and neither is the happy path.
 *
 * A `read()` argument the dataset never declared has to fail *at parse time*.
 * The aiwatcher API rejects unknown query parameters rather than ignoring
 * them, so forwarding one turns the whole query into somebody else's 400 with
 * nothing pointing at the word that caused it.
 *
 * And a misspelled *value* has to fail too. `hub: 'huggingfaec'` forwarded
 * verbatim comes back as an empty page, which reads as "there are no matching
 * datasets" — a statement about the world produced by a typo.
 */
final class HubDatasetTest extends TestCase
{
    private function catalog(FakeApi $api): Catalog
    {
        return new Catalog($api, 'http://aiwatcher.test');
    }

    private function api(): FakeApi
    {
        return new FakeApi([
            '/api/v1/dataset-hubs/search' => [[
                'results' => [
                    [
                        'hub' => 'huggingface',
                        'id' => 'someone/curated-corpus',
                        'title' => 'Curated corpus',
                        'owner' => 'someone',
                        'url' => 'https://huggingface.co/datasets/someone/curated-corpus',
                        'claimed_license' => 'mit',
                        'usage' => 'non_commercial',
                        'curated_source' => 'curated-corpus',
                        'downloads' => 4210,
                        'tags' => ['license:mit'],
                    ],
                    [
                        'hub' => 'kaggle',
                        'id' => 'someone/example-corpus',
                        'title' => 'Example corpus',
                        'owner' => 'someone',
                        'url' => 'https://www.kaggle.com/datasets/someone/example-corpus',
                        'claimed_license' => 'CC0: Public Domain',
                        'usage' => 'unclear',
                        'downloads' => 88,
                        'tags' => [],
                    ],
                ],
            ]],
            '/api/v1/dataset-hubs/images' => [[
                'hub' => 'huggingface',
                'dataset' => 'someone/floor-plans',
                'config' => 'default',
                'split' => 'train',
                'images' => [
                    [
                        'uri' => 'https://datasets-server.huggingface.co/a.jpg?Expires=1',
                        'width' => 1080,
                        'height' => 1537,
                        'row_index' => 0,
                        'column' => 'image',
                        'caption' => 'A floor plan of a house.',
                        'image_key' => 'someone/floor-plans/0',
                    ],
                    [
                        'uri' => 'https://datasets-server.huggingface.co/b.jpg?Expires=1',
                        'width' => 900,
                        'height' => 1200,
                        'row_index' => 1,
                        'column' => 'image',
                        'caption' => '',
                        'image_key' => 'someone/floor-plans/1',
                    ],
                ],
            ]],
            '/api/v1/annotation-images' => [[
                'images' => [
                    [
                        'project' => 'corpora/first',
                        'review' => 'accepted',
                        'image' => [
                            'image_id' => \str_repeat('ab', 32),
                            'uri' => 'aiwatcher://blob/' . \str_repeat('ab', 32),
                            'width' => 1064,
                            'height' => 1021,
                            'group_id' => 'subject-a',
                            'source' => 'example',
                            'level' => 'primary',
                        ],
                    ],
                ],
            ]],
        ]);
    }

    private function rows(string $query, FakeApi $api): array
    {
        $plan = (new PipelineBuilder($this->catalog($api)))->build(Parser::parse($query));

        return $plan->frame->fetch()->toArray();
    }

    public function test_a_hub_search_comes_back_as_flat_rows(): void
    {
        $api = $this->api();
        $rows = $this->rows("data_frame()->read(hub_datasets, q: 'floor plan')->fetch()", $api);

        self::assertCount(2, $rows);
        self::assertSame('someone/curated-corpus', $rows[0]['id']);
        self::assertSame('mit', $rows[0]['claimed_license']);
        // The verdict and the mirror's claim are separate columns, and the
        // column names say which is which.
        self::assertSame('non_commercial', $rows[0]['usage']);
        self::assertSame('curated-corpus', $rows[0]['curated_source']);
    }

    public function test_the_search_reaches_the_route_rather_than_filtering_afterwards(): void
    {
        $api = $this->api();
        $this->rows("data_frame()->read(hub_datasets, q: 'floor plan', hub: 'kaggle')->fetch()", $api);

        // `q`, which is how this route spells it — the annotation routes spell
        // theirs `search`, and a parameter declared under the wrong one reaches
        // aiwatcher as a 400 naming a word nobody wrote. FakeApi answers on the
        // path alone, so this assertion is the only thing that sees it.
        self::assertStringContainsString('q=floor+plan', $api->requested[0]);
        self::assertStringContainsString('hub=kaggle', $api->requested[0]);
    }

    public function test_the_images_dataset_reads_one_row_per_picture(): void
    {
        $api = $this->api();
        $rows = $this->rows("data_frame()->read(hub_images, dataset: 'someone/floor-plans')->fetch()", $api);

        self::assertCount(2, $rows);
        self::assertSame('https://datasets-server.huggingface.co/a.jpg?Expires=1', $rows[0]['uri']);
        self::assertSame(1080, $rows[0]['width']);
        // The per-image family key, which the import pipeline writes group_id
        // from. Composed by the API, never by the file name.
        self::assertSame('someone/floor-plans/0', $rows[0]['image_key']);
        self::assertStringContainsString('dataset=someone%2Ffloor-plans', $api->requested[0]);
    }

    public function test_an_argument_the_dataset_never_declared_is_a_parse_error(): void
    {
        $this->expectException(ParseError::class);
        // Forwarded, this would be a 400 from the aiwatcher API about the
        // whole request, with nothing naming the word that caused it.
        $this->rows("data_frame()->read(hub_datasets, licence: 'mit')->fetch()", $this->api());
    }

    public function test_a_misspelled_hub_is_a_parse_error_and_not_an_empty_result(): void
    {
        $api = $this->api();

        try {
            $this->rows("data_frame()->read(hub_datasets, hub: 'huggingfaec')->fetch()", $api);
            self::fail('a typo in a closed value set must not read as "no matches"');
        } catch (ParseError $error) {
            self::assertStringContainsString('huggingface', $error->getMessage());
            self::assertStringContainsString('kaggle', $error->getMessage());
        }

        self::assertSame([], $api->requested, 'and it must not have been asked');
    }

    public function test_the_annotation_list_needs_a_project_and_says_what_to_write(): void
    {
        try {
            $this->rows('data_frame()->read(annotation_images)->fetch()', $this->api());
            self::fail('there is no route without a project');
        } catch (ParseError $error) {
            self::assertStringContainsString('read(annotation_images, project:', $error->getMessage());
        }
    }

    public function test_an_image_head_is_flattened_into_the_columns_it_declares(): void
    {
        $api = $this->api();
        $rows = $this->rows("data_frame()->read(annotation_images, project: 'corpora/first')->fetch()", $api);

        self::assertCount(1, $rows);
        // `ref('group_id')`, not `array_get(ref('image'), 'group_id')`.
        self::assertSame('subject-a', $rows[0]['group_id']);
        self::assertSame('accepted', $rows[0]['review']);
        self::assertSame(1064, $rows[0]['width']);
    }

    public function test_the_family_column_is_explained_to_whoever_asks_for_an_image_split(): void
    {
        $catalog = $this->catalog($this->api());
        $dataset = $catalog->resolve('annotation_images');
        self::assertNotNull($dataset);

        $advice = $dataset->explainUnknownColumn('family');
        self::assertStringContainsString('group_id', $advice);
        self::assertStringContainsString('subject', $advice);
    }
}
