<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Dataset;

use Flow\ETL\Exception\DataDependentSchemaException;
use Flow\ETL\FlowContext;
use Flow\ETL\Pipeline\BoundStep;
use Flow\ETL\Rows;
use Flow\ETL\Schema;
use Flow\ETL\Transformer;
use Flow\ETL\Transformer\ScalarFunctionTransformer;
use Flow\Types\Type;
use Flow\Types\Type\TypeFactory;
use Flow\Types\Type\TypeWidener;

use function Flow\ETL\DSL\cast;
use function Flow\ETL\DSL\ref;
use function Flow\Types\DSL\structure_element;
use function Flow\Types\DSL\type_boolean;
use function Flow\Types\DSL\type_float;
use function Flow\Types\DSL\type_integer;
use function Flow\Types\DSL\type_list;
use function Flow\Types\DSL\type_null;
use function Flow\Types\DSL\type_optional;
use function Flow\Types\DSL\type_string;
use function Flow\Types\DSL\type_structure;

/** The HTTP adapter declares response_body as text, while 0.44's array_get needs
 * a structure. Describe each bounded HTTP page, including dynamic hub fields,
 * then let the native scalar transformer decode it. No dataset is collected and
 * no input Rows are rebuilt; the catalog supplies fields omitted by the API.
 */
final readonly class DecodeResponse implements Transformer
{
    /** @param array<string, string> $columns API path => catalog type */
    public function __construct(
        private string $rowsPath,
        private array $columns,
    ) {}

    public function bind(Schema $input): BoundStep
    {
        throw DataDependentSchemaException::step('HTTP JSON decoding', 'hub field types come from the response page');
    }

    public function transform(Rows $rows, FlowContext $context): Rows
    {
        if ($rows->count() === 0) {
            return $rows;
        }
        $type = type_null();
        $widener = new TypeWidener();
        foreach ($rows as $row) {
            $body = \json_decode($row->get('response_body'), true, 512, \JSON_THROW_ON_ERROR);
            $type = $widener->widen($type, $this->describe($body, ''));
        }
        return (new ScalarFunctionTransformer('__body', cast(ref('response_body'), $type)))->transform($rows, $context);
    }

    private function describe(mixed $value, string $path): Type
    {
        if ($path === $this->rowsPath) {
            $element = $this->recordType([]);
            foreach ($value as $record) {
                $element = (new TypeWidener())->widen($element, $this->recordType($record));
            }
            return type_list($element);
        }
        if (\is_array($value)) {
            if (\array_is_list($value)) {
                $element = type_null();
                foreach ($value as $item) {
                    $element = (new TypeWidener())->widen($element, $this->describe($item, $path . '[]'));
                }
                return type_list($element);
            }
            $elements = [];
            foreach ($value as $name => $item) {
                $elements[$name] = structure_element(
                    $name,
                    $this->describe($item, $path === '' ? (string) $name : $path . '.' . $name),
                    true,
                );
            }
            return type_structure($elements);
        }
        return match (true) {
            $value === null => type_null(),
            \is_int($value) => type_integer(),
            \is_float($value) => type_float(),
            \is_bool($value) => type_boolean(),
            default => type_string(),
        };
    }

    private function recordType(array $record): Type
    {
        $tree = [];
        foreach ($this->columns as $path => $declared) {
            $node = &$tree;
            $value = $record;
            $parts = \explode('.', $path);
            $leaf = \array_pop($parts);
            foreach ($parts as $part) {
                $node = &$node[$part];
                $value = $value[$part] ?? [];
            }
            $node[$leaf] = $declared === 'array'
                ? $this->describe($value[$leaf] ?? null, '__dynamic')
                : type_optional(
                    $declared === 'list<string>'
                        ? type_list(type_string())
                        : TypeFactory::fromString(\str_replace('|null', '', $declared)),
                );
            unset($node);
        }
        return $this->structure($tree);
    }

    private function structure(array $tree): Type
    {
        $elements = [];
        foreach ($tree as $name => $type) {
            $elements[$name] = structure_element($name, \is_array($type) ? $this->structure($type) : $type, true);
        }
        return type_structure($elements, allow_extra: true);
    }
}
