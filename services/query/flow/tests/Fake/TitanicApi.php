<?php

declare(strict_types=1);

namespace Aiwatcher\Flow\Tests\Fake;

use Nyholm\Psr7\Response;
use Psr\Http\Client\ClientInterface;
use Psr\Http\Message\RequestInterface;
use Psr\Http\Message\ResponseInterface;

/**
 * The first passengers of `phihung/titanic`, as the hub rows route serves them.
 *
 * One page: this route pages by offset and reports no cursor, and a corpus is
 * read in one bite here rather than walked.
 */
final class TitanicApi implements ClientInterface
{
    private const array PASSENGERS = [
        [
            'PassengerId' => 1,
            'Survived' => 0,
            'Pclass' => 3,
            'Name' => 'Braund, Mr. Owen Harris',
            'Sex' => 'male',
            'Age' => 22.0,
            'SibSp' => 1,
            'Parch' => 0,
            'Ticket' => 'A/5 21171',
            'Fare' => 7.25,
            'Cabin' => null,
            'Embarked' => 'S',
        ],
        [
            'PassengerId' => 2,
            'Survived' => 1,
            'Pclass' => 1,
            'Name' => 'Cumings, Mrs. John Bradley (Florence Briggs Thayer)',
            'Sex' => 'female',
            'Age' => 38.0,
            'SibSp' => 1,
            'Parch' => 0,
            'Ticket' => 'PC 17599',
            'Fare' => 71.2833,
            'Cabin' => 'C85',
            'Embarked' => 'C',
        ],
        [
            'PassengerId' => 3,
            'Survived' => 1,
            'Pclass' => 3,
            'Name' => 'Heikkinen, Miss. Laina',
            'Sex' => 'female',
            'Age' => 26.0,
            'SibSp' => 0,
            'Parch' => 0,
            'Ticket' => 'STON/O2. 3101282',
            'Fare' => 7.925,
            'Cabin' => null,
            'Embarked' => 'S',
        ],
        [
            'PassengerId' => 6,
            'Survived' => 0,
            'Pclass' => 3,
            'Name' => 'Moran, Mr. James',
            'Sex' => 'male',
            'Age' => null,
            'SibSp' => 0,
            'Parch' => 0,
            'Ticket' => '330877',
            'Fare' => 8.4583,
            'Cabin' => null,
            'Embarked' => 'Q',
        ],
        [
            'PassengerId' => 8,
            'Survived' => 0,
            'Pclass' => 3,
            'Name' => 'Palsson, Master. Gosta Leonard',
            'Sex' => 'male',
            'Age' => 2.0,
            'SibSp' => 3,
            'Parch' => 1,
            'Ticket' => '349909',
            'Fare' => 21.075,
            'Cabin' => null,
            'Embarked' => 'S',
        ],
        [
            'PassengerId' => 15,
            'Survived' => 0,
            'Pclass' => 3,
            'Name' => 'Vestrom, Miss. Hulda Amanda Adolfina',
            'Sex' => 'female',
            'Age' => 14.0,
            'SibSp' => 0,
            'Parch' => 0,
            'Ticket' => '350406',
            'Fare' => 7.8542,
            'Cabin' => null,
            'Embarked' => 'S',
        ],
        [
            'PassengerId' => 31,
            'Survived' => 0,
            'Pclass' => 1,
            'Name' => 'Uruchurtu, Don. Manuel E',
            'Sex' => 'male',
            'Age' => 40.0,
            'SibSp' => 0,
            'Parch' => 0,
            'Ticket' => 'PC 17601',
            'Fare' => 27.7208,
            'Cabin' => null,
            'Embarked' => 'C',
        ],
        [
            'PassengerId' => 370,
            'Survived' => 1,
            'Pclass' => 1,
            'Name' => 'Aubart, Mme. Leontine Pauline',
            'Sex' => 'female',
            'Age' => 24.0,
            'SibSp' => 0,
            'Parch' => 0,
            'Ticket' => 'PC 17477',
            'Fare' => 69.3,
            'Cabin' => 'B35',
            'Embarked' => 'C',
        ],
        [
            'PassengerId' => 444,
            'Survived' => 1,
            'Pclass' => 2,
            'Name' => 'Reynaldo, Ms. Encarnacion',
            'Sex' => 'female',
            'Age' => 28.0,
            'SibSp' => 0,
            'Parch' => 0,
            'Ticket' => '230434',
            'Fare' => 13.0,
            'Cabin' => null,
            'Embarked' => 'S',
        ],
        [
            'PassengerId' => 642,
            'Survived' => 1,
            'Pclass' => 1,
            'Name' => 'Sagesser, Mlle. Emma',
            'Sex' => 'female',
            'Age' => 24.0,
            'SibSp' => 0,
            'Parch' => 0,
            'Ticket' => 'PC 17477',
            'Fare' => 69.3,
            'Cabin' => 'B35',
            'Embarked' => 'C',
        ],
    ];

    public function sendRequest(RequestInterface $request): ResponseInterface
    {
        $body = [
            'hub' => 'huggingface',
            'dataset' => 'phihung/titanic',
            'config' => 'default',
            'split' => 'train',
            'columns' => \array_map(static fn(string $name): array => [
                'name' => $name,
                'kind' => 'Value',
                'dtype' => '',
            ], \array_keys(self::PASSENGERS[0])),
            'rows' => \array_map(
                static fn(int $index, array $passenger): array => ['row_index' => $index, 'row' => $passenger],
                \array_keys(self::PASSENGERS),
                self::PASSENGERS,
            ),
        ];

        return new Response(200, ['content-type' => 'application/json'], \json_encode($body, \JSON_THROW_ON_ERROR));
    }
}
