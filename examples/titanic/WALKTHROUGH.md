# Titanic krok po kroku: FlowPHP + FlowAI

Ten wariant przygotowuje dane w PHP. FlowAI dostarcza podział danych, uzupełnianie
braków i encodery. Python wykonuje wizualizację oraz trening Random Forest.
Gotowy plik do importu: [titanic-php.flow.json](titanic-php.flow.json).
Instalacja i uruchomienie od zera: [README](README.md#recommended-flowphp--flowai).
Wszystkie bloki można też otworzyć jako komórki: [widok notebooka i własny kod](NOTEBOOK.md).

Poniżej są rzeczywiste zrzuty panelu z 9 września 2026. Każdy krok pokazuje
konfigurację konkretnego bloku. Tabela po lewej pokazuje **końcowy wynik całego
flow**, nie wynik pośredni zaznaczonego bloku. Bloki PHP wykonują się razem jako
jedno zapytanie; ich liczniki dotyczą tego wspólnego wykonania. Zrzuty źródła
i importu wykonano przed uruchomieniem, pozostałe po przetworzeniu 891 rekordów.

## 0. Import przykładu

Otwórz **Data Curation → Pipeline → Import flow** i wybierz
`examples/titanic/titanic-php.flow.json`. Pojawi się szkic `curation/titanic-flowphp`:
10 bloków, 9 połączeń, kod transformacji PHP i dwa źródła Pythona.
Import sprawdza sumy kontrolne źródeł. Sam nie uruchamia kodu.
Alternatywnie wybierz **curation/titanic-flowphp** z **Saved pipelines**.

![Zaimportowany flow z blokami tematycznymi](screenshots/00-import.png)

## 1. Source — surowi pasażerowie

Kliknij **Titanic · raw passengers**. Ustaw dataset `hub_rows`, źródło
`phihung/titanic`, split `train`, limit `891`. Serwis pobiera kolejne strony
po maksymalnie 100 rekordów. Wyjściowe pola pasażera są zagnieżdżone w `row`.

![Konfiguracja źródła danych](screenshots/01-source.png)

## 2. Select raw columns — jawny zestaw kolumn

Ten blok przenosi oryginalne 12 pól z `row` na najwyższy poziom.
`PassengerId` identyfikuje rekord, `Survived` jest celem. `Age`, `Fare`, `Cabin`
i `Embarked` pozostają surowe — nie ukrywamy ich braków.
Kod można edytować w inspektorze po prawej.

![Wybór kolumn w FlowPHP](screenshots/02-columns.png)

## 3. Train / validation split — podział przed uczeniem

`trainTestSplit('PassengerId', target: 'Survived', fraction: 0.2, seed: 42)`
dodaje `_split`. Dla pełnego zbioru daje 712 rekordów `train` i 179 `validation`.
Podział uwzględnia klasy celu i jest deterministyczny przy przestawieniu kolejności
wejścia. Wymaga unikalnych identyfikatorów i przynajmniej dwóch rekordów klasy.

![Podział treningowy i walidacyjny w PHP](screenshots/03-split.png)

## 4. Missing values — uzupełnianie braków

`imputeMissing` uczy się median wyłącznie na `fitOn: '_split'`, domyślnie
`fitValue: 'train'`. Wiek uzupełnia według `Sex,Pclass`, opłatę według `Pclass`,
a port według najczęstszej wartości treningowej. Nieznana grupa korzysta
ze statystyki całego treningu. Powstają `AgeFilled`, `FareFilled`,
`EmbarkedFilled` i flagi braków. Oryginalne kolumny zostają do kontroli.

![Uzupełnianie braków z dopasowaniem na treningu](screenshots/04-missing-values.png)

## 5. Feature engineering — budowanie cech w FlowPHP

Tworzymy `Title`, `Deck`, `FamilySize`, `IsAlone`, `FarePerPerson` i
`TicketFrequency`. Częstość biletu zlicza tylko pasażerów treningowych;
nieznany bilet dostaje wartość 1. Hash biletu służy jako techniczny klucz
partycji, ponieważ niektóre numery zawierają `/`. Oryginalny `Ticket` zostaje.
To zwykłe transformacje FlowPHP, które można rozbudować we własnym flow.

![Budowa cech pasażera w FlowPHP](screenshots/05-features.png)

## 6. OneHotEncoder — cechy kategoryczne w PHP

`oneHotEncode` dopasowuje słownik `Sex`, `Pclass`, `EmbarkedFilled`, `Title`
i `Deck` do części treningowej. Zapisuje numeryczne wskaźniki w `model_features`.
Nieznana kategoria walidacyjna daje zera (`handleUnknown: 'ignore'`).
`stateOutput: 'feature_encoder'` zachowuje słownik wraz z wierszami.
Można go później przekazać jako `state: '<JSON>'`, aby tylko transformować dane.

![OneHotEncoder dostępny bezpośrednio w zapytaniu PHP](screenshots/06-one-hot.png)

## 7. LabelEncoder — kodowanie celu w PHP

`labelEncode('Survived', fitOn: '_split')` tworzy `target_encoded`.
W tym zbiorze klasy 0 i 1 pozostają odpowiednio 0 i 1. Nieznana klasa powoduje
błąd. Stan w `label_encoder` pozwala odtworzyć mapowanie; klasę PHP można też
wywołać samodzielnie przez `fit`, `transform` i `inverseTransform`.
Cel nie trafia do cech modelu.

![LabelEncoder i zapis jego stanu](screenshots/07-label.png)

## 8. Visualization — wykresy treningu

Pierwszy blok Pythona pokazuje liczbę surowych braków i przeżywalność według płci.
Wykresy korzystają tylko z treningu. Blok przekazuje wszystkie 891 wierszy dalej,
bez ich filtrowania. Kliknij **Visualization** i przewiń inspektor do podglądu
Live. Źródło wykresu jest edytowalne i dołączane do eksportu.

![Rzeczywiste wykresy w podglądzie Visualization](screenshots/08-visualization.png)

## 9. Model training & evaluation — model i wynik walidacyjny

Random Forest używa jawnie wybranych cech numerycznych oraz `model_features`.
Uczy się na `train`, ocenia wyłącznie `validation`, a predykcje dodaje do każdego
wiersza jako `prediction` i `survival_probability`. Dla pełnego zbioru oraz
zapisanych zależności: **accuracy 0,837989, ROC AUC 0,885310**.

To wynik jednego podziału kontrolnego. Rodziny i wspólne bilety mogą występować
po obu stronach podziału; nie jest to ocena generalizacji na nowe rodziny ani
wynik konkursowego test.csv. **Preview 25 rows** służy do sprawdzenia połączeń.

![Raport modelu na 179 pasażerach walidacyjnych](screenshots/09-model.png)

## 10. Publish predictions — docelowy dataset

Blok **Publish predictions** określa nazwę `curation/titanic-flowphp`.
**Run** daje podgląd wyniku. Osobne **Publish** zapisuje wersję datasetu.
Zrzut pokazuje konfigurację gotową do publikacji, nie potwierdzenie publikacji.
**Save** zapisuje definicję flow i przypina rewizje notebooków. Formularz
Schedule jest osobną funkcją: samo zapisanie pipeline nie włącza harmonogramu.

![Konfiguracja datasetu wynikowego](screenshots/10-publish.png)

## 11. Eksport i dalsza praca

Kliknij **Export flow with code**. Paczka zawiera kod PHP w blokach `transform`,
źródła dwóch notebooków, parametry, połączenia i pozycje. Otwórz ją ponownie przez
**Import flow** na instancji z zainstalowanymi bibliotekami FlowAI.
Zapisany stan encoderów jest częścią danych wynikowych; paczka flow przenosi
przepis i kod, a nie wytrenowany model czy snapshot pasażerów.

![Ukończony flow i komunikat eksportu](screenshots/11-export.png)

Kod tej wersji jest również w [pipeline-php.json](pipeline-php.json),
a kompletne zapytanie w [preparation.flow](preparation.flow).
Wariant offline uruchomisz przez `run.py --php --csv /path/to/train.csv`.
Lokalny runner zapisuje wykres, metryki, predykcje oraz oba stany encoderów.
